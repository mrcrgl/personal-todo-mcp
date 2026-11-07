//! Markdown files and the text operations on them. Paths here are relative to
//! the files directory; where that directory lives is the adapter's concern.

use crate::domain::todo::ValidationError;

/// Leaves headroom under GitHub's 1 MB limit for reading contents as JSON.
pub const MAX_FILE_BYTES: usize = 512 * 1024;
const MAX_PATH_CHARS: usize = 200;
const MAX_DEPTH: usize = 8;
const MAX_EDITS: usize = 20;
const MAX_LINE_CHARS: usize = 2_000;
pub const DEFAULT_WINDOW_LINES: usize = 400;
pub const MAX_WINDOW_LINES: usize = 2_000;
/// Context shown around a change, and the most lines an edit result shows.
const SNIPPET_CONTEXT: usize = 3;
const MAX_SNIPPET_LINES: usize = 40;
/// Characters that break checkouts on some systems or carry path meaning.
const FORBIDDEN_CHARS: &[char] = &['\\', ':', '*', '?', '"', '<', '>', '|'];
const EXTENSION: &str = ".md";

/// A Markdown file, e.g. `projects/house.md`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FilePath(String);

impl FilePath {
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        let path = value.trim();
        if path.is_empty() {
            return Err(ValidationError("path must not be empty".into()));
        }
        check_segments(path)?;
        if !path.ends_with(EXTENSION) {
            return Err(ValidationError(
                "only Markdown files are supported: the path must end in .md".into(),
            ));
        }
        Ok(Self(path.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    pub fn is_within(&self, folder: &FolderPath) -> bool {
        folder.0.is_empty()
            || self
                .0
                .strip_prefix(folder.0.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    }
}

/// A folder to list. Empty means all files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderPath(String);

impl FolderPath {
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        let path = value.trim();
        let path = path.strip_suffix('/').unwrap_or(path);
        if path.is_empty() {
            return Ok(Self::default());
        }
        check_segments(path)?;
        Ok(Self(path.to_string()))
    }
}

/// Every path rule except the extension. A path that passes cannot leave the
/// files directory or reach a hidden one such as `.github`.
fn check_segments(path: &str) -> Result<(), ValidationError> {
    if path.chars().count() > MAX_PATH_CHARS {
        return Err(ValidationError(format!(
            "path is longer than {MAX_PATH_CHARS} characters"
        )));
    }
    if path.starts_with('/') {
        return Err(ValidationError(
            "path must be relative, e.g. notes/ideas.md".into(),
        ));
    }
    if path
        .chars()
        .any(|c| c.is_control() || FORBIDDEN_CHARS.contains(&c))
    {
        return Err(ValidationError(
            "path must not contain control characters or any of \\ : * ? \" < > |".into(),
        ));
    }
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() > MAX_DEPTH {
        return Err(ValidationError(format!(
            "path is nested deeper than {MAX_DEPTH} levels"
        )));
    }
    for segment in segments {
        if segment.is_empty() {
            return Err(ValidationError(
                "path must not contain empty segments".into(),
            ));
        }
        if segment.starts_with('.') {
            return Err(ValidationError(
                "path segments must not start with a dot".into(),
            ));
        }
        if segment.trim() != segment {
            return Err(ValidationError(
                "path segments must not start or end with spaces".into(),
            ));
        }
    }
    Ok(())
}

/// File content as written by a tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileText(String);

impl FileText {
    pub fn parse(value: String) -> Result<Self, ValidationError> {
        check_text(&value).map_err(|error| ValidationError(error.to_string()))?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TextProblem {
    #[error("file is larger than {} KB", MAX_FILE_BYTES / 1024)]
    TooLarge,
    #[error("file contains NUL characters, so it is not text")]
    Nul,
}

fn check_text(value: &str) -> Result<(), TextProblem> {
    if value.len() > MAX_FILE_BYTES {
        return Err(TextProblem::TooLarge);
    }
    if value.contains('\0') {
        return Err(TextProblem::Nul);
    }
    Ok(())
}

/// A stored file and the version it was read at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    pub path: FilePath,
    pub sha: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: FilePath,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileListing {
    pub files: Vec<FileEntry>,
    /// GitHub returned a partial tree.
    pub truncated: bool,
}

/// What a write expects to find, so a stale write fails instead of
/// overwriting someone else's change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expected {
    Absent,
    Version(String),
}

/// A numbered line of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub number: usize,
    pub text: String,
}

/// Part of a file, by line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineWindow {
    pub lines: Vec<Line>,
    pub total_lines: usize,
}

impl LineWindow {
    /// `offset` is 1-based. Carriage returns are dropped and over-long lines
    /// are cut, so the output stays readable whatever the file holds.
    pub fn of(text: &str, offset: usize, limit: usize) -> Self {
        let offset = offset.max(1);
        let lines = text
            .lines()
            .enumerate()
            .skip(offset - 1)
            .take(limit)
            .map(|(index, line)| Line {
                number: index + 1,
                text: cut_line(line),
            })
            .collect();
        Self {
            lines,
            total_lines: text.lines().count(),
        }
    }

    pub fn last_number(&self) -> Option<usize> {
        self.lines.last().map(|line| line.number)
    }
}

fn cut_line(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        return line.to_string();
    }
    let kept: String = line.chars().take(MAX_LINE_CHARS).collect();
    format!("{kept}… [line cut]")
}

/// One exact replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub old: String,
    pub new: String,
    pub replace_all: bool,
}

/// Messages name the edit by position but never echo file content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("give at least one edit")]
    NoEdits,
    #[error("give at most {MAX_EDITS} edits per call")]
    TooManyEdits,
    #[error("edit {0}: old_string must not be empty; use file_write to create a file")]
    EmptyOld(usize),
    #[error("edit {0}: old_string and new_string are the same")]
    Unchanged(usize),
    #[error(
        "edit {0}: old_string was not found. It must match the file exactly, including \
         whitespace and indentation. Read the file again and copy the text from it"
    )]
    NoMatch(usize),
    #[error(
        "edit {index}: old_string matches {count} times. Include more surrounding text to \
         make it unique, or set replace_all"
    )]
    Ambiguous { index: usize, count: usize },
    #[error("the edited {0}")]
    Text(TextProblem),
}

impl Edit {
    /// For a file with Windows line endings, matched as the file has them.
    fn with_crlf(&self) -> Self {
        Self {
            old: to_crlf(&self.old),
            new: to_crlf(&self.new),
            replace_all: self.replace_all,
        }
    }
}

fn to_crlf(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// Checks the edits on their own, before anything is read.
pub fn check_edits(edits: &[Edit]) -> Result<(), EditError> {
    if edits.is_empty() {
        return Err(EditError::NoEdits);
    }
    if edits.len() > MAX_EDITS {
        return Err(EditError::TooManyEdits);
    }
    for (index, edit) in edits.iter().enumerate() {
        if edit.old.is_empty() {
            return Err(EditError::EmptyOld(index + 1));
        }
        if edit.old == edit.new {
            return Err(EditError::Unchanged(index + 1));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edited {
    pub text: FileText,
    pub replacements: usize,
}

/// Applies edits in order, each to the result of the one before. Either all
/// apply or none do.
pub fn apply_edits(text: &str, edits: &[Edit]) -> Result<Edited, EditError> {
    check_edits(edits)?;
    let crlf = text.contains("\r\n");
    let mut current = text.to_string();
    let mut replacements = 0;
    for (index, edit) in edits.iter().enumerate() {
        let number = index + 1;
        let mut count = current.matches(edit.old.as_str()).count();
        let mut edit = edit.clone();
        if count == 0 && crlf {
            let converted = edit.with_crlf();
            count = current.matches(converted.old.as_str()).count();
            edit = converted;
        }
        match count {
            0 => return Err(EditError::NoMatch(number)),
            1 => {}
            _ if edit.replace_all => {}
            _ => {
                return Err(EditError::Ambiguous {
                    index: number,
                    count,
                });
            }
        }
        current = current.replace(edit.old.as_str(), &edit.new);
        replacements += count;
    }
    check_text(&current).map_err(EditError::Text)?;
    Ok(Edited {
        text: FileText(current),
        replacements,
    })
}

/// The lines of `after` that differ from `before`, with some context, so an
/// agent can check an edit without reading the file again.
pub fn changed_lines(before: &str, after: &str) -> LineWindow {
    let old: Vec<&str> = before.lines().collect();
    let new: Vec<&str> = after.lines().collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    let first_changed = prefix + 1;
    let last_changed = (new.len() - suffix).max(first_changed);
    let start = first_changed.saturating_sub(SNIPPET_CONTEXT).max(1);
    let end = last_changed + SNIPPET_CONTEXT;
    let limit = (end + 1 - start).min(MAX_SNIPPET_LINES);
    LineWindow::of(after, start, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(old: &str, new: &str) -> Edit {
        Edit {
            old: old.into(),
            new: new.into(),
            replace_all: false,
        }
    }

    #[test]
    fn relative_markdown_paths_are_accepted() {
        for path in [
            "ideas.md",
            "projects/house.md",
            " a b/c d.md ",
            "über/ärger.md",
        ] {
            assert!(FilePath::parse(path).is_ok(), "{path}");
        }
        assert_eq!(
            FilePath::parse(" notes/x.md ").unwrap().as_str(),
            "notes/x.md"
        );
    }

    #[test]
    fn paths_that_could_leave_the_files_folder_are_refused() {
        for path in [
            "",
            "/etc/passwd.md",
            "../secret.md",
            "notes/../../secret.md",
            "./x.md",
            ".github/workflows/ci.md",
            "notes/.hidden.md",
            "notes//x.md",
            "notes\\x.md",
            "C:x.md",
            "notes/x.md/",
            "a\u{0}b.md",
            "a\nb.md",
            "notes/ x.md",
        ] {
            assert!(FilePath::parse(path).is_err(), "{path:?} was accepted");
        }
    }

    #[test]
    fn only_markdown_files_are_accepted() {
        assert!(FilePath::parse("script.sh").is_err());
        assert!(FilePath::parse("notes").is_err());
        assert!(FilePath::parse(".md").is_err());
    }

    #[test]
    fn long_or_deep_paths_are_refused() {
        assert!(FilePath::parse(&format!("{}.md", "a".repeat(200))).is_err());
        assert!(FilePath::parse("a/b/c/d/e/f/g/h/i.md").is_err());
        assert!(FilePath::parse("a/b/c/d/e/f/g/h.md").is_ok());
    }

    #[test]
    fn folders_may_be_empty_or_end_in_a_slash() {
        assert_eq!(FolderPath::parse("").unwrap(), FolderPath::default());
        assert_eq!(FolderPath::parse("/").unwrap(), FolderPath::default());
        let folder = FolderPath::parse("projects/").unwrap();
        assert!(FilePath::parse("projects/a.md").unwrap().is_within(&folder));
        assert!(
            !FilePath::parse("projects2/a.md")
                .unwrap()
                .is_within(&folder)
        );
        assert!(
            FilePath::parse("a.md")
                .unwrap()
                .is_within(&FolderPath::default())
        );
        assert!(FolderPath::parse("../x").is_err());
        assert!(FolderPath::parse(".github").is_err());
    }

    #[test]
    fn text_is_limited_in_size_and_must_not_contain_nul() {
        assert!(FileText::parse("# Notes\n".into()).is_ok());
        assert!(FileText::parse("a".repeat(MAX_FILE_BYTES + 1)).is_err());
        assert!(FileText::parse("a\0b".into()).is_err());
    }

    #[test]
    fn a_window_numbers_lines_from_its_offset() {
        let window = LineWindow::of("a\nb\r\nc\nd\n", 2, 2);
        assert_eq!(window.total_lines, 4);
        assert_eq!(
            window.lines,
            [
                Line {
                    number: 2,
                    text: "b".into()
                },
                Line {
                    number: 3,
                    text: "c".into()
                },
            ]
        );
        assert_eq!(window.last_number(), Some(3));
    }

    #[test]
    fn a_window_past_the_end_is_empty() {
        let window = LineWindow::of("a\nb", 10, 5);
        assert!(window.lines.is_empty());
        assert_eq!(window.total_lines, 2);
        assert_eq!(LineWindow::of("", 1, 5).total_lines, 0);
    }

    #[test]
    fn over_long_lines_are_cut() {
        let window = LineWindow::of(&"x".repeat(5_000), 1, 1);
        assert!(window.lines[0].text.ends_with("[line cut]"));
        assert!(window.lines[0].text.chars().count() < 2_100);
    }

    #[test]
    fn edits_apply_in_order() {
        let edited = apply_edits(
            "# Plan\n- one\n- two\n",
            &[
                edit("- one", "- first"),
                edit("- first\n- two", "- first\n- second"),
            ],
        )
        .unwrap();
        assert_eq!(edited.text.as_str(), "# Plan\n- first\n- second\n");
        assert_eq!(edited.replacements, 2);
    }

    #[test]
    fn a_missing_match_names_the_edit_and_changes_nothing() {
        let err = apply_edits("abc", &[edit("a", "x"), edit("zzz", "y")]).unwrap_err();
        assert_eq!(err, EditError::NoMatch(2));
        assert!(err.to_string().contains("edit 2"));
        assert!(!err.to_string().contains("zzz"), "messages echo no content");
    }

    #[test]
    fn an_ambiguous_match_needs_replace_all() {
        let err = apply_edits("- [ ] a\n- [ ] b\n", &[edit("- [ ]", "- [x]")]).unwrap_err();
        assert_eq!(err, EditError::Ambiguous { index: 1, count: 2 });

        let all = Edit {
            replace_all: true,
            ..edit("- [ ]", "- [x]")
        };
        let edited = apply_edits("- [ ] a\n- [ ] b\n", &[all]).unwrap();
        assert_eq!(edited.text.as_str(), "- [x] a\n- [x] b\n");
        assert_eq!(edited.replacements, 2);
    }

    #[test]
    fn edits_match_files_with_windows_line_endings() {
        let edited = apply_edits("a\r\nb\r\nc\r\n", &[edit("a\nb", "a\nB")]).unwrap();
        assert_eq!(edited.text.as_str(), "a\r\nB\r\nc\r\n");
    }

    #[test]
    fn useless_edits_are_refused_before_reading() {
        assert_eq!(check_edits(&[]), Err(EditError::NoEdits));
        assert_eq!(check_edits(&[edit("", "x")]), Err(EditError::EmptyOld(1)));
        assert_eq!(
            check_edits(&[edit("a", "b"), edit("x", "x")]),
            Err(EditError::Unchanged(2))
        );
        assert_eq!(
            check_edits(&vec![edit("a", "b"); 21]),
            Err(EditError::TooManyEdits)
        );
    }

    #[test]
    fn an_edit_cannot_grow_a_file_past_the_limit() {
        let err = apply_edits("x", &[edit("x", &"y".repeat(MAX_FILE_BYTES + 1))]).unwrap_err();
        assert_eq!(err, EditError::Text(TextProblem::TooLarge));
    }

    #[test]
    fn changed_lines_show_the_change_with_context() {
        let before: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        let after = before.replace("line 10\n", "line ten\nline ten and a half\n");
        let window = changed_lines(&before, &after);
        let numbers: Vec<usize> = window.lines.iter().map(|l| l.number).collect();
        assert_eq!(numbers, (7..=14).collect::<Vec<_>>());
        assert_eq!(window.lines[3].text, "line ten");
    }

    #[test]
    fn changed_lines_handle_deletions_at_the_end() {
        let window = changed_lines("a\nb\nc\n", "a\nb\n");
        assert_eq!(window.total_lines, 2);
        assert_eq!(window.last_number(), Some(2));
    }
}
