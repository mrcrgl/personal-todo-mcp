//! Table storage port. A table is loaded and saved whole, under one version,
//! so a schema and its rows can never be written apart.

use std::future::Future;
use std::pin::Pin;

use crate::domain::table::model::{MAX_TABLE_BYTES, Table};
use crate::domain::table::names::TableName;
use crate::domain::tenant::{RepositoryProblem, Tenant};
use crate::domain::version::Expected;

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = Result<T, TableStoreError>> + Send + 'a>>;

/// Messages are shown to the agent. They name no table and echo no content.
#[derive(Debug, thiserror::Error)]
pub enum TableStoreError {
    #[error("the table does not exist: table_list shows the tables there are")]
    NotFound,
    #[error("a table with this name already exists")]
    AlreadyExists,
    #[error("the table changed since it was read: read it again and retry")]
    Conflict,
    #[error(
        "the table would be larger than {} KB: delete rows or split it into several tables",
        MAX_TABLE_BYTES / 1024
    )]
    TooLarge,
    /// The stored file does not parse or no longer fits its schema, e.g.
    /// after a hand edit. `line` counts from 1.
    #[error(
        "the stored table is damaged at line {line}: {problem}. Fix that line in the \
         repository, or drop the table"
    )]
    Damaged { line: usize, problem: String },
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

impl TableStoreError {
    /// A stable, content-free name for logs.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::AlreadyExists => "already_exists",
            Self::Conflict => "conflict",
            Self::TooLarge => "too_large",
            Self::Damaged { .. } => "damaged",
            Self::PermissionNotGranted => "permission_not_granted",
            Self::InstallationGone => "installation_gone",
            Self::Repository(_) => "repository",
            Self::RateLimited => "rate_limited",
            Self::Rejected { .. } => "rejected",
            Self::Unavailable(_) => "unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableEntry {
    pub name: TableName,
    /// Stored size in bytes.
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableListing {
    pub tables: Vec<TableEntry>,
    /// The store returned a partial listing.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredTable {
    pub table: Table,
    pub sha: String,
}

pub trait TableStore: Send + Sync {
    fn list<'a>(&'a self, tenant: &'a Tenant) -> BoxFut<'a, TableListing>;
    fn read<'a>(&'a self, tenant: &'a Tenant, name: &'a TableName) -> BoxFut<'a, StoredTable>;
    /// Writes the whole table and returns its new `sha`. Fails with
    /// `Conflict` or `AlreadyExists` when the table is not as `expected`, and
    /// with `TooLarge` before writing a table over the limit.
    fn write<'a>(
        &'a self,
        tenant: &'a Tenant,
        name: &'a TableName,
        table: &'a Table,
        expected: &'a Expected,
    ) -> BoxFut<'a, String>;
    fn delete<'a>(
        &'a self,
        tenant: &'a Tenant,
        name: &'a TableName,
        sha: &'a str,
    ) -> BoxFut<'a, ()>;
}
