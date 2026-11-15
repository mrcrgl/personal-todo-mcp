//! `TableStore` on the repository's contents: the Contents API for a table,
//! the Git Trees API for listing. Uses the same contents-scoped token as the
//! file store.

use std::future::Future;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::domain::table::model::Table;
use crate::domain::table::names::TableName;
use crate::domain::tenant::Tenant;
use crate::domain::version::Expected;
use crate::github::api::GitHubApi;
use crate::github::installation::{InstallationAccess, InstallationAccessProvider, Scope};
use crate::github::models::{ContentRow, ContentWriteRow, TreeRow};
use crate::github::table_mapping::{self, table_error};
use crate::tables::store::{BoxFut, StoredTable, TableListing, TableStore, TableStoreError};
use crate::todos::store::StoreError;

pub struct GitHubTableStore {
    api: GitHubApi,
    installations: Arc<InstallationAccessProvider>,
}

impl GitHubTableStore {
    pub fn new(api_url: String, installations: Arc<InstallationAccessProvider>) -> Self {
        Self {
            api: GitHubApi::new(api_url),
            installations,
        }
    }

    /// Runs one operation with the tenant's contents access. Every path to
    /// GitHub goes through here, so no operation can name a repository.
    async fn with_access<'a, T, F, Fut>(
        &'a self,
        tenant: &Tenant,
        operation: F,
    ) -> Result<T, TableStoreError>
    where
        F: FnOnce(Arc<InstallationAccess>) -> Fut,
        Fut: Future<Output = Result<T, TableStoreError>> + 'a,
    {
        let access = self
            .installations
            .access(tenant.installation_id, Scope::Contents)
            .await
            .map_err(table_error)?;
        let result = operation(access).await;
        if matches!(result, Err(TableStoreError::InstallationGone)) {
            self.installations.invalidate(tenant.installation_id).await;
        }
        result
    }
}

/// Kept as segments so the client encodes each one: a segment can never add
/// a level. The name is a `TableName`, so it holds no `/` anyway.
fn contents_path<'a>(access: &'a InstallationAccess, file_name: &'a str) -> [&'a str; 6] {
    let repo = &access.repository;
    [
        "repos",
        repo.owner.login.as_str(),
        repo.name.as_str(),
        "contents",
        table_mapping::ROOT,
        file_name,
    ]
}

/// A create that finds a file is refused with 422 because no `sha` was given.
fn write_error(error: StoreError, expected: &Expected) -> TableStoreError {
    match (error, expected) {
        (StoreError::Rejected { status: 422, .. }, Expected::Absent) => {
            TableStoreError::AlreadyExists
        }
        (error, _) => table_error(error),
    }
}

/// An empty repository has no tree, and a missing branch none either.
fn list_error(error: StoreError) -> Result<TableListing, TableStoreError> {
    match error {
        StoreError::Rejected {
            status: 404 | 409, ..
        } => Ok(TableListing {
            tables: Vec::new(),
            truncated: false,
        }),
        other => Err(table_error(other)),
    }
}

impl TableStore for GitHubTableStore {
    fn list<'a>(&'a self, tenant: &'a Tenant) -> BoxFut<'a, TableListing> {
        Box::pin(self.with_access(tenant, move |access| async move {
            let repo = &access.repository;
            let tree: Result<TreeRow, StoreError> = self
                .api
                .get(
                    &access.token,
                    &[
                        "repos",
                        &repo.owner.login,
                        &repo.name,
                        "git",
                        "trees",
                        &repo.default_branch,
                    ],
                    &[("recursive", "1".to_string())],
                )
                .await;
            match tree {
                Ok(tree) => Ok(table_mapping::listing_from_tree(tree)),
                Err(error) => list_error(error),
            }
        }))
    }

    fn read<'a>(&'a self, tenant: &'a Tenant, name: &'a TableName) -> BoxFut<'a, StoredTable> {
        Box::pin(self.with_access(tenant, move |access| async move {
            let file_name = table_mapping::file_name(name);
            // A folder answers with an array, so the shape is checked first.
            let value: Value = self
                .api
                .get(&access.token, &contents_path(&access, &file_name), &[])
                .await
                .map_err(table_error)?;
            if !value.is_object() {
                return Err(TableStoreError::NotFound);
            }
            let row: ContentRow = serde_json::from_value(value)
                .map_err(|_| TableStoreError::Unavailable("unreadable GitHub response".into()))?;
            let sha = row.sha.clone();
            let table = table_mapping::table_from_content(row)?;
            Ok(StoredTable { table, sha })
        }))
    }

    fn write<'a>(
        &'a self,
        tenant: &'a Tenant,
        name: &'a TableName,
        table: &'a Table,
        expected: &'a Expected,
    ) -> BoxFut<'a, String> {
        Box::pin(async move {
            // Checked before any request, so an oversized table costs nothing.
            let content = table_mapping::encoded_content(table)?;
            self.with_access(tenant, move |access| async move {
                let file_name = table_mapping::file_name(name);
                let repository_path = table_mapping::repository_path(name);
                let mut body = json!({"content": content});
                match expected {
                    Expected::Absent => {
                        body["message"] = json!(format!("Create {repository_path}"));
                    }
                    Expected::Version(sha) => {
                        body["message"] = json!(format!("Update {repository_path}"));
                        body["sha"] = json!(sha);
                    }
                }
                let row: ContentWriteRow = self
                    .api
                    .put(&access.token, &contents_path(&access, &file_name), &body)
                    .await
                    .map_err(|error| write_error(error, expected))?;
                Ok(row.content.sha)
            })
            .await
        })
    }

    fn delete<'a>(
        &'a self,
        tenant: &'a Tenant,
        name: &'a TableName,
        sha: &'a str,
    ) -> BoxFut<'a, ()> {
        Box::pin(self.with_access(tenant, move |access| async move {
            let file_name = table_mapping::file_name(name);
            let message = format!("Delete {}", table_mapping::repository_path(name));
            self.api
                .delete_with_body(
                    &access.token,
                    &contents_path(&access, &file_name),
                    &json!({"message": message, "sha": sha}),
                )
                .await
                .map_err(table_error)
        }))
    }
}
