//! File use cases.

use std::sync::Arc;

use crate::domain::file::{
    Edit, EditError, Expected, FileListing, FilePath, FileText, FolderPath, LineWindow,
    apply_edits, changed_lines, check_edits,
};
use crate::domain::tenant::Tenant;
use crate::files::store::{FileStore, FileStoreError};

/// How often an edit is tried when the file changes underneath it. One retry
/// covers a concurrent writer; more would hide a real fight over the file.
const EDIT_ATTEMPTS: usize = 2;

#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error(transparent)]
    Store(#[from] FileStoreError),
    #[error(transparent)]
    Edit(#[from] EditError),
}

impl FileError {
    /// A stable, content-free name for logs.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Store(error) => error.kind(),
            Self::Edit(_) => "edit_rejected",
        }
    }
}

#[derive(Debug)]
pub struct FileView {
    pub path: FilePath,
    pub sha: String,
    pub window: LineWindow,
}

#[derive(Debug)]
pub struct WriteOutcome {
    pub path: FilePath,
    pub sha: String,
    pub created: bool,
}

#[derive(Debug)]
pub struct EditOutcome {
    pub path: FilePath,
    pub sha: String,
    pub replacements: usize,
    pub changed: LineWindow,
}

pub struct FileService {
    store: Arc<dyn FileStore>,
}

impl FileService {
    pub fn new(store: Arc<dyn FileStore>) -> Self {
        Self { store }
    }

    pub async fn list(
        &self,
        tenant: &Tenant,
        folder: &FolderPath,
    ) -> Result<FileListing, FileError> {
        let mut listing = self.store.list(tenant, folder).await?;
        listing.files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(listing)
    }

    pub async fn read(
        &self,
        tenant: &Tenant,
        path: &FilePath,
        offset: usize,
        limit: usize,
    ) -> Result<FileView, FileError> {
        let file = self.store.read(tenant, path).await?;
        Ok(FileView {
            window: LineWindow::of(&file.text, offset, limit),
            path: file.path,
            sha: file.sha,
        })
    }

    /// Without a `sha`, creates the file and refuses to replace one.
    pub async fn write(
        &self,
        tenant: &Tenant,
        path: &FilePath,
        text: &FileText,
        sha: Option<String>,
    ) -> Result<WriteOutcome, FileError> {
        let created = sha.is_none();
        let expected = sha.map_or(Expected::Absent, Expected::Version);
        let sha = self.store.write(tenant, path, text, &expected).await?;
        Ok(WriteOutcome {
            path: path.clone(),
            sha,
            created,
        })
    }

    /// Edits are anchored on text, not on a version, so re-applying them to a
    /// file someone else just changed is safe: they either still match
    /// exactly or fail.
    pub async fn edit(
        &self,
        tenant: &Tenant,
        path: &FilePath,
        edits: &[Edit],
    ) -> Result<EditOutcome, FileError> {
        check_edits(edits)?;
        let mut attempt = 1;
        loop {
            let file = self.store.read(tenant, path).await?;
            let edited = apply_edits(&file.text, edits)?;
            let expected = Expected::Version(file.sha);
            match self
                .store
                .write(tenant, path, &edited.text, &expected)
                .await
            {
                Ok(sha) => {
                    return Ok(EditOutcome {
                        path: path.clone(),
                        sha,
                        replacements: edited.replacements,
                        changed: changed_lines(&file.text, edited.text.as_str()),
                    });
                }
                Err(FileStoreError::Conflict) if attempt < EDIT_ATTEMPTS => attempt += 1,
                Err(error) => return Err(error.into()),
            }
        }
    }

    pub async fn delete(
        &self,
        tenant: &Tenant,
        path: &FilePath,
        sha: &str,
    ) -> Result<(), FileError> {
        Ok(self.store.delete(tenant, path, sha).await?)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::domain::file::{FileEntry, StoredFile};
    use crate::files::store::BoxFut;

    /// An in-memory store whose file can be changed "by someone else" between
    /// a read and a write.
    struct MemoryStore {
        file: Mutex<(String, String)>,
        changes_before_write: Mutex<Vec<String>>,
        writes: Mutex<usize>,
    }

    impl MemoryStore {
        fn new(text: &str) -> Self {
            Self {
                file: Mutex::new((text.into(), "v1".into())),
                changes_before_write: Mutex::new(Vec::new()),
                writes: Mutex::new(0),
            }
        }
    }

    impl FileStore for MemoryStore {
        fn list<'a>(&'a self, _: &'a Tenant, _: &'a FolderPath) -> BoxFut<'a, FileListing> {
            Box::pin(async {
                Ok(FileListing {
                    files: vec![
                        FileEntry {
                            path: FilePath::parse("b.md").unwrap(),
                            size: 1,
                        },
                        FileEntry {
                            path: FilePath::parse("a.md").unwrap(),
                            size: 1,
                        },
                    ],
                    truncated: false,
                })
            })
        }

        fn read<'a>(&'a self, _: &'a Tenant, path: &'a FilePath) -> BoxFut<'a, StoredFile> {
            Box::pin(async move {
                let (text, sha) = self.file.lock().unwrap().clone();
                Ok(StoredFile {
                    path: path.clone(),
                    sha,
                    text,
                })
            })
        }

        fn write<'a>(
            &'a self,
            _: &'a Tenant,
            _: &'a FilePath,
            text: &'a FileText,
            expected: &'a Expected,
        ) -> BoxFut<'a, String> {
            Box::pin(async move {
                *self.writes.lock().unwrap() += 1;
                let mut file = self.file.lock().unwrap();
                if let Some(change) = self.changes_before_write.lock().unwrap().pop() {
                    *file = (change, format!("{}x", file.1));
                }
                if *expected != Expected::Version(file.1.clone()) {
                    return Err(FileStoreError::Conflict);
                }
                *file = (text.as_str().into(), format!("{}+", file.1));
                Ok(file.1.clone())
            })
        }

        fn delete<'a>(&'a self, _: &'a Tenant, _: &'a FilePath, _: &'a str) -> BoxFut<'a, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    fn tenant() -> Tenant {
        Tenant {
            user_id: 1,
            login: "alice".into(),
            installation_id: 1,
        }
    }

    fn edit(old: &str, new: &str) -> Edit {
        Edit {
            old: old.into(),
            new: new.into(),
            replace_all: false,
        }
    }

    fn path() -> FilePath {
        FilePath::parse("notes.md").unwrap()
    }

    #[tokio::test]
    async fn an_edit_writes_against_the_version_it_read() {
        let store = Arc::new(MemoryStore::new("a\nb\n"));
        let service = FileService::new(store.clone());

        let outcome = service
            .edit(&tenant(), &path(), &[edit("b", "B")])
            .await
            .unwrap();

        assert_eq!(outcome.sha, "v1+");
        assert_eq!(outcome.replacements, 1);
        assert_eq!(store.file.lock().unwrap().0, "a\nB\n");
    }

    #[tokio::test]
    async fn an_edit_is_retried_once_on_a_concurrent_change() {
        let store = Arc::new(MemoryStore::new("a\nb\n"));
        store
            .changes_before_write
            .lock()
            .unwrap()
            .push("a\nb\nc\n".into());
        let service = FileService::new(store.clone());

        service
            .edit(&tenant(), &path(), &[edit("b", "B")])
            .await
            .unwrap();

        assert_eq!(store.file.lock().unwrap().0, "a\nB\nc\n");
        assert_eq!(*store.writes.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn a_second_conflict_is_reported() {
        let store = Arc::new(MemoryStore::new("a\nb\n"));
        store
            .changes_before_write
            .lock()
            .unwrap()
            .extend(["a\nb\nd\n".to_string(), "a\nb\nc\n".to_string()]);
        let service = FileService::new(store.clone());

        let err = service
            .edit(&tenant(), &path(), &[edit("b", "B")])
            .await
            .unwrap_err();

        assert!(matches!(err, FileError::Store(FileStoreError::Conflict)));
        assert_eq!(*store.writes.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn invalid_edits_are_refused_before_reading() {
        let store = Arc::new(MemoryStore::new("a"));
        let service = FileService::new(store.clone());

        let err = service.edit(&tenant(), &path(), &[]).await.unwrap_err();

        assert!(matches!(err, FileError::Edit(EditError::NoEdits)));
        assert_eq!(*store.writes.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn a_write_without_sha_expects_no_file() {
        let store = Arc::new(MemoryStore::new("a"));
        let service = FileService::new(store);
        let text = FileText::parse("b".into()).unwrap();

        let err = service
            .write(&tenant(), &path(), &text, None)
            .await
            .unwrap_err();
        assert!(matches!(err, FileError::Store(FileStoreError::Conflict)));

        let outcome = service
            .write(&tenant(), &path(), &text, Some("v1".into()))
            .await
            .unwrap();
        assert!(!outcome.created);
    }

    #[tokio::test]
    async fn listings_are_sorted_by_path() {
        let service = FileService::new(Arc::new(MemoryStore::new("")));
        let listing = service
            .list(&tenant(), &FolderPath::default())
            .await
            .unwrap();
        let paths: Vec<&str> = listing.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.md", "b.md"]);
    }
}
