//! File storage port. Every operation takes the tenant it acts for, so an
//! implementation cannot be called without one.

use std::future::Future;
use std::pin::Pin;

use crate::domain::file::{Expected, FileListing, FilePath, FileText, FolderPath, StoredFile};
use crate::domain::tenant::{RepositoryProblem, Tenant};

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = Result<T, FileStoreError>> + Send + 'a>>;

/// Messages are shown to the agent. They name no path and echo no content,
/// since the agent already knows both and logs must not carry them.
#[derive(Debug, thiserror::Error)]
pub enum FileStoreError {
    #[error("the file does not exist")]
    NotFound,
    #[error(
        "the file already exists: read it and pass its sha to replace it, or change it with \
         file_edit"
    )]
    AlreadyExists,
    #[error("the file changed since it was read: read it again and retry")]
    Conflict,
    #[error("the stored file is larger than {} KB", crate::domain::file::MAX_FILE_BYTES / 1024)]
    TooLarge,
    #[error("the stored file is not UTF-8 text")]
    NotText,
    #[error(
        "the GitHub App has no access to repository contents yet: approve its updated \
         permissions in your GitHub settings under Applications → Installed GitHub Apps, then \
         try again"
    )]
    PermissionNotGranted,
    #[error("the GitHub App is no longer installed: reconnect to install it again")]
    InstallationGone,
    #[error("{0}")]
    Repository(RepositoryProblem),
    #[error("GitHub rate limit reached: try again later")]
    RateLimited,
    /// `message` is GitHub's own. It may name the path, so it is never logged.
    #[error("GitHub rejected the request ({status}): {message}")]
    Rejected { status: u16, message: String },
    #[error("GitHub could not be reached: {0}")]
    Unavailable(String),
}

impl FileStoreError {
    /// A stable, content-free name for logs.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::AlreadyExists => "already_exists",
            Self::Conflict => "conflict",
            Self::TooLarge => "too_large",
            Self::NotText => "not_text",
            Self::PermissionNotGranted => "permission_not_granted",
            Self::InstallationGone => "installation_gone",
            Self::Repository(_) => "repository",
            Self::RateLimited => "rate_limited",
            Self::Rejected { .. } => "rejected",
            Self::Unavailable(_) => "unavailable",
        }
    }
}

pub trait FileStore: Send + Sync {
    /// Markdown files in `folder` and below, in no particular order.
    fn list<'a>(&'a self, tenant: &'a Tenant, folder: &'a FolderPath) -> BoxFut<'a, FileListing>;
    fn read<'a>(&'a self, tenant: &'a Tenant, path: &'a FilePath) -> BoxFut<'a, StoredFile>;
    /// Writes the whole file and returns its new `sha`. Fails with `Conflict`
    /// or `AlreadyExists` when the file is not as `expected`.
    fn write<'a>(
        &'a self,
        tenant: &'a Tenant,
        path: &'a FilePath,
        text: &'a FileText,
        expected: &'a Expected,
    ) -> BoxFut<'a, String>;
    fn delete<'a>(&'a self, tenant: &'a Tenant, path: &'a FilePath, sha: &'a str)
    -> BoxFut<'a, ()>;
}
