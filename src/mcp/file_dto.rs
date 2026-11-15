//! File tool arguments and results, with explicit mapping to and from the
//! domain. Results an agent reads line by line are rendered as text.

use std::fmt::Write as _;

use rmcp::schemars;
use serde::{Deserialize, Serialize};

use crate::domain::file::{
    DEFAULT_WINDOW_LINES, Edit, FileListing, FilePath, FileText, FolderPath, LineWindow,
    MAX_WINDOW_LINES,
};
use crate::domain::todo::ValidationError;
use crate::files::service::{EditOutcome, FileView, WriteOutcome};

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct FileListArgs {
    /// Only files in this folder and below, e.g. "projects". Defaults to all files.
    pub folder: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FileReadArgs {
    /// File path, e.g. "projects/house.md".
    pub path: String,
    /// First line to read, counted from 1. Defaults to 1.
    pub offset: Option<u32>,
    /// Number of lines to read. Defaults to 400, at most 2000.
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EditArg {
    /// Exact text to replace, including whitespace and indentation, without
    /// line numbers. Must occur exactly once unless replace_all is set.
    pub old_string: String,
    /// Text to put in its place. May be empty to delete old_string.
    pub new_string: String,
    /// Replace every occurrence instead of requiring exactly one.
    pub replace_all: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FileEditArgs {
    /// File path, e.g. "projects/house.md".
    pub path: String,
    /// Applied in order, each to the result of the one before. Either all
    /// apply or none do.
    pub edits: Vec<EditArg>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FileWriteArgs {
    /// File path, e.g. "projects/house.md". Folders are created as needed.
    pub path: String,
    /// The whole file, as Markdown.
    pub content: String,
    /// The sha from file_read, to replace an existing file. Omit to create a new one.
    pub sha: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FileDeleteArgs {
    /// File path, e.g. "projects/house.md".
    pub path: String,
    /// The sha from file_read, so a file changed since is not deleted.
    pub sha: String,
}

/// A read request in domain terms.
pub struct ReadRequest {
    pub path: FilePath,
    pub offset: usize,
    pub limit: usize,
}

impl FileListArgs {
    pub fn into_folder(self) -> Result<FolderPath, ValidationError> {
        FolderPath::parse(self.folder.as_deref().unwrap_or_default())
    }
}

impl FileReadArgs {
    pub fn into_request(self) -> Result<ReadRequest, ValidationError> {
        let limit = self
            .limit
            .map_or(DEFAULT_WINDOW_LINES, |limit| limit as usize);
        Ok(ReadRequest {
            path: FilePath::parse(&self.path)?,
            offset: self.offset.unwrap_or(1).max(1) as usize,
            limit: limit.clamp(1, MAX_WINDOW_LINES),
        })
    }
}

impl FileEditArgs {
    pub fn into_edits(self) -> Result<(FilePath, Vec<Edit>), ValidationError> {
        let path = FilePath::parse(&self.path)?;
        let edits = self
            .edits
            .into_iter()
            .map(|edit| Edit {
                old: edit.old_string,
                new: edit.new_string,
                replace_all: edit.replace_all.unwrap_or(false),
            })
            .collect();
        Ok((path, edits))
    }
}

impl FileWriteArgs {
    pub fn into_write(self) -> Result<(FilePath, FileText, Option<String>), ValidationError> {
        Ok((
            FilePath::parse(&self.path)?,
            FileText::parse(self.content)?,
            self.sha.map(|sha| sha.trim().to_string()),
        ))
    }
}

impl FileDeleteArgs {
    pub fn into_delete(self) -> Result<(FilePath, String), ValidationError> {
        let sha = self.sha.trim().to_string();
        if sha.is_empty() {
            return Err(ValidationError("sha must not be empty".into()));
        }
        Ok((FilePath::parse(&self.path)?, sha))
    }
}

#[derive(Debug, Serialize)]
pub struct FileEntryDto {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct FileListingDto {
    pub files: Vec<FileEntryDto>,
    pub truncated: bool,
}

impl From<FileListing> for FileListingDto {
    fn from(listing: FileListing) -> Self {
        Self {
            files: listing
                .files
                .into_iter()
                .map(|entry| FileEntryDto {
                    path: entry.path.as_str().to_string(),
                    size: entry.size,
                })
                .collect(),
            truncated: listing.truncated,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct WriteDto {
    pub path: String,
    pub sha: String,
    pub created: bool,
}

impl From<WriteOutcome> for WriteDto {
    fn from(outcome: WriteOutcome) -> Self {
        Self {
            path: outcome.path.as_str().to_string(),
            sha: outcome.sha,
            created: outcome.created,
        }
    }
}

/// `cat -n` style, which agents read reliably and can quote from without
/// mistaking line numbers for content.
fn write_lines(out: &mut String, window: &LineWindow) {
    for line in &window.lines {
        let _ = writeln!(out, "{:>6}\t{}", line.number, line.text);
    }
}

pub fn render_view(view: &FileView, offset: usize) -> String {
    let window = &view.window;
    let mut out = format!("path: {}\nsha: {}\n", view.path.as_str(), view.sha);
    match window.last_number() {
        _ if window.total_lines == 0 => out.push_str("The file is empty.\n"),
        None => {
            let _ = writeln!(
                out,
                "No lines from offset {offset}: the file has {} lines.",
                window.total_lines
            );
        }
        Some(last) => {
            let first = window.lines[0].number;
            let _ = write!(out, "lines {first}-{last} of {}", window.total_lines);
            if last < window.total_lines {
                let _ = write!(out, ". More follows: read on with offset {}", last + 1);
            }
            out.push_str(".\n\n");
            write_lines(&mut out, window);
        }
    }
    out
}

pub fn render_edit(outcome: &EditOutcome) -> String {
    let plural = if outcome.replacements == 1 { "" } else { "s" };
    let mut out = format!(
        "Edited {}: {} replacement{plural}.\nsha: {}\n",
        outcome.path.as_str(),
        outcome.replacements,
        outcome.sha
    );
    if outcome.changed.lines.is_empty() {
        out.push_str("The file is now empty.\n");
    } else {
        out.push_str("Changed lines with context:\n\n");
        write_lines(&mut out, &outcome.changed);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(text: &str, offset: usize, limit: usize) -> FileView {
        FileView {
            path: FilePath::parse("a.md").unwrap(),
            sha: "abc".into(),
            window: LineWindow::of(text, offset, limit),
        }
    }

    #[test]
    fn reads_default_to_the_start_and_clamp_the_limit() {
        let request = FileReadArgs {
            path: "a.md".into(),
            offset: Some(0),
            limit: Some(100_000),
        }
        .into_request()
        .unwrap();
        assert_eq!(request.offset, 1);
        assert_eq!(request.limit, 2_000);

        let request = FileReadArgs {
            path: "a.md".into(),
            offset: None,
            limit: None,
        }
        .into_request()
        .unwrap();
        assert_eq!((request.offset, request.limit), (1, 400));
    }

    #[test]
    fn a_partial_read_says_where_to_continue() {
        let text = render_view(&view("one\ntwo\nthree\n", 1, 2), 1);
        assert_eq!(
            text,
            "path: a.md\nsha: abc\nlines 1-2 of 3. More follows: read on with offset 3.\n\n     1\tone\n     2\ttwo\n"
        );
    }

    #[test]
    fn empty_files_and_reads_past_the_end_are_explained() {
        assert!(render_view(&view("", 1, 10), 1).contains("The file is empty."));
        assert!(render_view(&view("a\n", 5, 10), 5).contains("the file has 1 lines"));
    }

    #[test]
    fn invalid_arguments_are_refused() {
        assert!(
            FileDeleteArgs {
                path: "a.md".into(),
                sha: " ".into()
            }
            .into_delete()
            .is_err()
        );
        assert!(
            FileWriteArgs {
                path: "../a.md".into(),
                content: String::new(),
                sha: None
            }
            .into_write()
            .is_err()
        );
        assert!(
            FileListArgs {
                folder: Some(".github".into())
            }
            .into_folder()
            .is_err()
        );
    }
}
