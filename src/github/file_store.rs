//! `FileStore` on the repository's contents: the Contents API for single
//! files, the Git Trees API for listing.

use std::future::Future;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::domain::file::{Expected, FileListing, FilePath, FileText, FolderPath, StoredFile};
use crate::domain::tenant::Tenant;
use crate::files::store::{BoxFut, FileStore, FileStoreError};
use crate::github::api::GitHubApi;
use crate::github::file_mapping::{self, file_error};
use crate::github::installation::{InstallationAccess, InstallationAccessProvider, Scope};
use crate::github::models::{ContentRow, ContentWriteRow, TreeRow};
use crate::todos::store::StoreError;

pub struct GitHubFileStore {
    api: GitHubApi,
    installations: Arc<InstallationAccessProvider>,
}

impl GitHubFileStore {
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
    ) -> Result<T, FileStoreError>
    where
        F: FnOnce(Arc<InstallationAccess>) -> Fut,
        Fut: Future<Output = Result<T, FileStoreError>> + 'a,
    {
        let access = self
            .installations
            .access(tenant.installation_id, Scope::Contents)
            .await
            .map_err(file_error)?;
        let result = operation(access).await;
        if matches!(result, Err(FileStoreError::InstallationGone)) {
            self.installations.invalidate(tenant.installation_id).await;
        }
        result
    }
}

fn contents_path<'a>(access: &'a InstallationAccess, path: &'a FilePath) -> Vec<&'a str> {
    let repo = &access.repository;
    let mut segments = vec![
        "repos",
        repo.owner.login.as_str(),
        repo.name.as_str(),
        "contents",
    ];
    segments.extend(file_mapping::content_segments(path));
    segments
}

/// A create that finds a file is refused with 422 because no `sha` was given.
fn write_error(error: StoreError, expected: &Expected) -> FileStoreError {
    match (error, expected) {
        (StoreError::Rejected { status: 422, .. }, Expected::Absent) => {
            FileStoreError::AlreadyExists
        }
        (error, _) => file_error(error),
    }
}

/// An empty repository has no tree, and a missing branch none either.
fn list_error(error: StoreError) -> Result<FileListing, FileStoreError> {
    match error {
        StoreError::Rejected {
            status: 404 | 409, ..
        } => Ok(FileListing {
            files: Vec::new(),
            truncated: false,
        }),
        other => Err(file_error(other)),
    }
}

impl FileStore for GitHubFileStore {
    fn list<'a>(&'a self, tenant: &'a Tenant, folder: &'a FolderPath) -> BoxFut<'a, FileListing> {
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
                Ok(tree) => Ok(file_mapping::listing_from_tree(tree, folder)),
                Err(error) => list_error(error),
            }
        }))
    }

    fn read<'a>(&'a self, tenant: &'a Tenant, path: &'a FilePath) -> BoxFut<'a, StoredFile> {
        Box::pin(self.with_access(tenant, move |access| async move {
            // A folder answers with an array, so the shape is checked first.
            let value: Value = self
                .api
                .get(&access.token, &contents_path(&access, path), &[])
                .await
                .map_err(file_error)?;
            if !value.is_object() {
                return Err(FileStoreError::NotFound);
            }
            let row: ContentRow = serde_json::from_value(value)
                .map_err(|_| FileStoreError::Unavailable("unreadable GitHub response".into()))?;
            file_mapping::file_from_content(path, row)
        }))
    }

    fn write<'a>(
        &'a self,
        tenant: &'a Tenant,
        path: &'a FilePath,
        text: &'a FileText,
        expected: &'a Expected,
    ) -> BoxFut<'a, String> {
        Box::pin(self.with_access(tenant, move |access| async move {
            let repository_path = file_mapping::repository_path(path);
            let mut body = json!({"content": file_mapping::encode_text(text)});
            match expected {
                Expected::Absent => body["message"] = json!(format!("Create {repository_path}")),
                Expected::Version(sha) => {
                    body["message"] = json!(format!("Update {repository_path}"));
                    body["sha"] = json!(sha);
                }
            }
            let row: ContentWriteRow = self
                .api
                .put(&access.token, &contents_path(&access, path), &body)
                .await
                .map_err(|error| write_error(error, expected))?;
            Ok(row.content.sha)
        }))
    }

    fn delete<'a>(
        &'a self,
        tenant: &'a Tenant,
        path: &'a FilePath,
        sha: &'a str,
    ) -> BoxFut<'a, ()> {
        Box::pin(self.with_access(tenant, move |access| async move {
            let message = format!("Delete {}", file_mapping::repository_path(path));
            self.api
                .delete_with_body(
                    &access.token,
                    &contents_path(&access, path),
                    &json!({"message": message, "sha": sha}),
                )
                .await
                .map_err(file_error)
        }))
    }
}
