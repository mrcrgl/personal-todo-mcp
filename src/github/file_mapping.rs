//! Repository contents ↔ files. The files directory is a storage detail, so
//! its name appears here and nowhere else.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;

use crate::domain::file::{
    FileEntry, FileListing, FilePath, FileText, FolderPath, MAX_FILE_BYTES, StoredFile,
};
use crate::files::store::FileStoreError;
use crate::github::models::{ContentRow, TreeRow};
use crate::todos::store::StoreError;

/// Where files live in the repository.
pub const ROOT: &str = "files";

/// URL path segments of a file below `repos/{owner}/{repo}/contents`. Kept as
/// segments so the client encodes each one: a segment can never add a level.
pub fn content_segments(path: &FilePath) -> Vec<&str> {
    std::iter::once(ROOT).chain(path.segments()).collect()
}

/// The path as it appears in the repository, e.g. for commit messages.
pub fn repository_path(path: &FilePath) -> String {
    format!("{ROOT}/{}", path.as_str())
}

/// Keeps the Markdown files below `folder`. Entries that would not pass as a
/// tool argument are left out, so every listed path can be read.
pub fn listing_from_tree(row: TreeRow, folder: &FolderPath) -> FileListing {
    let prefix = format!("{ROOT}/");
    let files = row
        .tree
        .into_iter()
        .filter(|entry| entry.kind == "blob")
        .filter_map(|entry| {
            let relative = entry.path.strip_prefix(&prefix)?;
            let path = FilePath::parse(relative).ok()?;
            (path.as_str() == relative && path.is_within(folder)).then_some(FileEntry {
                path,
                size: entry.size.unwrap_or(0),
            })
        })
        .collect();
    FileListing {
        files,
        truncated: row.truncated,
    }
}

pub fn file_from_content(path: &FilePath, row: ContentRow) -> Result<StoredFile, FileStoreError> {
    if row.kind != "file" {
        return Err(FileStoreError::NotFound);
    }
    if row.size > MAX_FILE_BYTES as u64 {
        return Err(FileStoreError::TooLarge);
    }
    if row.encoding.as_deref() != Some("base64") {
        return Err(FileStoreError::Unavailable(
            "GitHub returned the file in an unexpected encoding".into(),
        ));
    }
    // GitHub wraps the base64 at 60 characters.
    let encoded: String = row
        .content
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    let bytes = B64
        .decode(encoded)
        .map_err(|_| FileStoreError::Unavailable("GitHub returned unreadable content".into()))?;
    let text = String::from_utf8(bytes).map_err(|_| FileStoreError::NotText)?;
    Ok(StoredFile {
        path: path.clone(),
        sha: row.sha,
        text,
    })
}

pub fn encode_text(text: &FileText) -> String {
    B64.encode(text.as_str())
}

/// The failures every file operation shares. Status codes whose meaning
/// depends on the operation are mapped by the caller first.
pub fn file_error(error: StoreError) -> FileStoreError {
    match error {
        StoreError::Rejected { status: 404, .. } | StoreError::NotFound(_) => {
            FileStoreError::NotFound
        }
        StoreError::Rejected { status: 409, .. } => FileStoreError::Conflict,
        StoreError::Rejected { status, message } => FileStoreError::Rejected { status, message },
        StoreError::PermissionNotGranted => FileStoreError::PermissionNotGranted,
        StoreError::InstallationGone => FileStoreError::InstallationGone,
        StoreError::Repository(problem) => FileStoreError::Repository(problem),
        StoreError::RateLimited => FileStoreError::RateLimited,
        StoreError::Unavailable(message) => FileStoreError::Unavailable(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::models::TreeEntryRow;

    fn entry(path: &str, kind: &str) -> TreeEntryRow {
        TreeEntryRow {
            path: path.into(),
            kind: kind.into(),
            size: Some(3),
        }
    }

    fn content(kind: &str, size: u64, content: &str) -> ContentRow {
        ContentRow {
            kind: kind.into(),
            sha: "abc".into(),
            size,
            encoding: Some("base64".into()),
            content: Some(content.into()),
        }
    }

    fn path(value: &str) -> FilePath {
        FilePath::parse(value).unwrap()
    }

    #[test]
    fn files_live_below_the_root_folder() {
        let file = path("projects/house.md");
        assert_eq!(content_segments(&file), ["files", "projects", "house.md"]);
        assert_eq!(repository_path(&file), "files/projects/house.md");
    }

    #[test]
    fn a_listing_keeps_readable_markdown_files_below_the_folder() {
        let tree = TreeRow {
            truncated: false,
            tree: vec![
                entry("README.md", "blob"),
                entry("files", "tree"),
                entry("files/a.md", "blob"),
                entry("files/projects", "tree"),
                entry("files/projects/b.md", "blob"),
                entry("files/projects/photo.png", "blob"),
                entry("files/.hidden/c.md", "blob"),
                entry("files/projects/ d.md", "blob"),
                entry("filesystem/e.md", "blob"),
            ],
        };

        let all = listing_from_tree(
            TreeRow {
                truncated: true,
                tree: tree.tree.iter().map(|e| entry(&e.path, &e.kind)).collect(),
            },
            &FolderPath::default(),
        );
        let paths: Vec<&str> = all.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.md", "projects/b.md"]);
        assert!(all.truncated);

        let projects = listing_from_tree(tree, &FolderPath::parse("projects").unwrap());
        let paths: Vec<&str> = projects.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["projects/b.md"]);
    }

    #[test]
    fn content_is_decoded_from_wrapped_base64() {
        let file =
            file_from_content(&path("a.md"), content("file", 8, "IyBOb3Rl\ncwo=\n")).unwrap();
        assert_eq!(file.text, "# Notes\n");
        assert_eq!(file.sha, "abc");
    }

    #[test]
    fn content_that_is_not_a_readable_text_file_is_refused() {
        assert!(matches!(
            file_from_content(&path("a.md"), content("dir", 0, "")),
            Err(FileStoreError::NotFound)
        ));
        assert!(matches!(
            file_from_content(&path("a.md"), content("file", 600_000, "")),
            Err(FileStoreError::TooLarge)
        ));
        assert!(matches!(
            file_from_content(&path("a.md"), content("file", 2, "//4=")),
            Err(FileStoreError::NotText)
        ));
    }

    #[test]
    fn shared_failures_map_to_file_errors() {
        let rejected = |status| StoreError::Rejected {
            status,
            message: "x".into(),
        };
        assert!(matches!(
            file_error(rejected(404)),
            FileStoreError::NotFound
        ));
        assert!(matches!(
            file_error(rejected(409)),
            FileStoreError::Conflict
        ));
        assert!(matches!(
            file_error(StoreError::PermissionNotGranted),
            FileStoreError::PermissionNotGranted
        ));
    }
}
