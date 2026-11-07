//! `TodoStore` on GitHub Issues.

use std::future::Future;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::domain::tenant::Tenant;
use crate::domain::todo::{
    LabelRef, NewTodo, StateFilter, Todo, TodoComment, TodoFilter, TodoId, TodoLabels, TodoPage,
    TodoRepository,
};
use crate::github::api::GitHubApi;
use crate::github::installation::{InstallationAccess, InstallationAccessProvider, Scope};
use crate::github::mapping;
use crate::github::models::{CommentRow, IssueRow, LabelRow};
use crate::todos::store::{BoxFut, StoreError, TodoStore};

const PAGE_SIZE: usize = 100;
/// Bounds the GitHub calls one list can cost when filters match little.
const MAX_PAGES: usize = 5;
const NEW_LABEL_COLOR: &str = "ededed";

pub struct GitHubTodoStore {
    api: GitHubApi,
    installations: Arc<InstallationAccessProvider>,
}

impl GitHubTodoStore {
    pub fn new(api_url: String, installations: Arc<InstallationAccessProvider>) -> Self {
        Self {
            api: GitHubApi::new(api_url),
            installations,
        }
    }

    /// Runs one operation with the tenant's access. Every path to GitHub goes
    /// through here, so no operation can name a repository of its own.
    async fn with_access<'a, T, F, Fut>(
        &'a self,
        tenant: &Tenant,
        operation: F,
    ) -> Result<T, StoreError>
    where
        F: FnOnce(Arc<InstallationAccess>) -> Fut,
        Fut: Future<Output = Result<T, StoreError>> + 'a,
    {
        let access = self
            .installations
            .access(tenant.installation_id, Scope::Issues)
            .await?;
        let result = operation(access).await;
        if matches!(result, Err(StoreError::InstallationGone)) {
            self.installations.invalidate(tenant.installation_id).await;
        }
        result
    }

    async fn fetch_issue(
        &self,
        access: &InstallationAccess,
        id: TodoId,
    ) -> Result<Todo, StoreError> {
        let repo = &access.repository;
        let number = id.0.to_string();
        let row: IssueRow = self
            .api
            .get(
                &access.token,
                &["repos", &repo.owner.login, &repo.name, "issues", &number],
                &[],
            )
            .await
            .map_err(|error| not_found_as(error, id))?;
        if row.pull_request.is_some() {
            return Err(StoreError::NotFound(id.0));
        }
        Ok(mapping::todo_from_issue(row))
    }

    async fn fetch_page(
        &self,
        access: &InstallationAccess,
        filter: &TodoFilter,
        page: usize,
    ) -> Result<Vec<IssueRow>, StoreError> {
        let repo = &access.repository;
        let mut query = vec![
            ("state", github_state(filter.state).to_string()),
            ("per_page", PAGE_SIZE.to_string()),
            ("page", page.to_string()),
        ];
        let labels =
            mapping::issue_labels(filter.priority, filter.list.as_ref(), filter.tag.as_slice());
        if !labels.is_empty() {
            query.push(("labels", labels.join(",")));
        }
        self.api
            .get(
                &access.token,
                &["repos", &repo.owner.login, &repo.name, "issues"],
                &query,
            )
            .await
    }

    /// GitHub creates no labels for us reliably, so missing ones are created
    /// first. Concurrent creation is harmless: "already exists" is ignored.
    async fn ensure_labels(
        &self,
        access: &InstallationAccess,
        wanted: &[String],
    ) -> Result<(), StoreError> {
        if wanted.is_empty() {
            return Ok(());
        }
        let repo = &access.repository;
        let path = ["repos", &repo.owner.login, &repo.name, "labels"];
        let existing: Vec<LabelRow> = self
            .api
            .get(&access.token, &path, &[("per_page", PAGE_SIZE.to_string())])
            .await?;
        for name in wanted {
            if !existing.iter().any(|label| &label.name == name) {
                self.api
                    .post_idempotent(
                        &access.token,
                        &path,
                        &json!({"name": name, "color": NEW_LABEL_COLOR}),
                    )
                    .await?;
            }
        }
        Ok(())
    }
}

fn github_state(state: StateFilter) -> &'static str {
    match state {
        StateFilter::Open => "open",
        StateFilter::Done | StateFilter::Dropped => "closed",
        StateFilter::All => "all",
    }
}

fn not_found_as(error: StoreError, id: TodoId) -> StoreError {
    match error {
        StoreError::Rejected { status: 404, .. } => StoreError::NotFound(id.0),
        other => other,
    }
}

fn issue_payload(todo: &Todo) -> Value {
    let (state, state_reason) = mapping::issue_state(todo.state);
    let mut body = json!({
        "title": todo.title.as_str(),
        "body": mapping::compose_body(&todo.notes, todo.due),
        "state": state,
        "labels": mapping::issue_labels(todo.priority, todo.list.as_ref(), &todo.tags),
    });
    if let Some(reason) = state_reason {
        body["state_reason"] = json!(reason);
    }
    body
}

impl TodoStore for GitHubTodoStore {
    fn list<'a>(&'a self, tenant: &'a Tenant, filter: &'a TodoFilter) -> BoxFut<'a, TodoPage> {
        Box::pin(self.with_access(tenant, move |access| async move {
            let mut todos = Vec::new();
            let mut truncated = false;
            for page in 1..=MAX_PAGES {
                let rows = self.fetch_page(&access, filter, page).await?;
                let exhausted = rows.len() < PAGE_SIZE;
                todos.extend(
                    rows.into_iter()
                        .filter(|row| row.pull_request.is_none())
                        .map(mapping::todo_from_issue)
                        .filter(|todo| filter.matches(todo)),
                );
                truncated = !exhausted;
                if exhausted || todos.len() >= filter.limit {
                    break;
                }
            }
            if todos.len() > filter.limit {
                todos.truncate(filter.limit);
                truncated = true;
            }
            Ok(TodoPage { todos, truncated })
        }))
    }

    fn get<'a>(&'a self, tenant: &'a Tenant, id: TodoId) -> BoxFut<'a, Todo> {
        Box::pin(self.with_access(tenant, move |access| async move {
            self.fetch_issue(&access, id).await
        }))
    }

    fn comments<'a>(&'a self, tenant: &'a Tenant, id: TodoId) -> BoxFut<'a, Vec<TodoComment>> {
        Box::pin(self.with_access(tenant, move |access| async move {
            let repo = &access.repository;
            let number = id.0.to_string();
            let rows: Vec<CommentRow> = self
                .api
                .get(
                    &access.token,
                    &[
                        "repos",
                        &repo.owner.login,
                        &repo.name,
                        "issues",
                        &number,
                        "comments",
                    ],
                    &[("per_page", PAGE_SIZE.to_string())],
                )
                .await
                .map_err(|error| not_found_as(error, id))?;
            Ok(rows.into_iter().map(mapping::comment_from_row).collect())
        }))
    }

    fn create<'a>(&'a self, tenant: &'a Tenant, todo: &'a NewTodo) -> BoxFut<'a, Todo> {
        Box::pin(self.with_access(tenant, move |access| async move {
            let labels = mapping::issue_labels(todo.priority, todo.list.as_ref(), &todo.tags);
            self.ensure_labels(&access, &labels).await?;
            let repo = &access.repository;
            let row: IssueRow = self
                .api
                .post(
                    &access.token,
                    &["repos", &repo.owner.login, &repo.name, "issues"],
                    &json!({
                        "title": todo.title.as_str(),
                        "body": mapping::compose_body(&todo.notes, todo.due),
                        "labels": labels,
                    }),
                )
                .await?;
            Ok(mapping::todo_from_issue(row))
        }))
    }

    fn save<'a>(&'a self, tenant: &'a Tenant, todo: &'a Todo) -> BoxFut<'a, Todo> {
        Box::pin(self.with_access(tenant, move |access| async move {
            let labels = mapping::issue_labels(todo.priority, todo.list.as_ref(), &todo.tags);
            self.ensure_labels(&access, &labels).await?;
            let repo = &access.repository;
            let number = todo.id.0.to_string();
            let row: IssueRow = self
                .api
                .patch(
                    &access.token,
                    &["repos", &repo.owner.login, &repo.name, "issues", &number],
                    &issue_payload(todo),
                )
                .await
                .map_err(|error| not_found_as(error, todo.id))?;
            Ok(mapping::todo_from_issue(row))
        }))
    }

    fn add_comment<'a>(
        &'a self,
        tenant: &'a Tenant,
        id: TodoId,
        body: &'a str,
    ) -> BoxFut<'a, TodoComment> {
        Box::pin(self.with_access(tenant, move |access| async move {
            let repo = &access.repository;
            let number = id.0.to_string();
            let row: CommentRow = self
                .api
                .post(
                    &access.token,
                    &[
                        "repos",
                        &repo.owner.login,
                        &repo.name,
                        "issues",
                        &number,
                        "comments",
                    ],
                    &json!({"body": body}),
                )
                .await
                .map_err(|error| not_found_as(error, id))?;
            Ok(mapping::comment_from_row(row))
        }))
    }

    fn labels<'a>(&'a self, tenant: &'a Tenant) -> BoxFut<'a, TodoLabels> {
        Box::pin(self.with_access(tenant, move |access| async move {
            let repo = &access.repository;
            let rows: Vec<LabelRow> = self
                .api
                .get(
                    &access.token,
                    &["repos", &repo.owner.login, &repo.name, "labels"],
                    &[("per_page", PAGE_SIZE.to_string())],
                )
                .await?;
            Ok(mapping::labels_summary(rows))
        }))
    }

    fn delete_label<'a>(&'a self, tenant: &'a Tenant, label: &'a LabelRef) -> BoxFut<'a, ()> {
        Box::pin(self.with_access(tenant, move |access| async move {
            let repo = &access.repository;
            let name = mapping::label_name(label);
            self.api
                .delete(
                    &access.token,
                    &["repos", &repo.owner.login, &repo.name, "labels", &name],
                )
                .await
        }))
    }

    fn repository<'a>(&'a self, tenant: &'a Tenant) -> BoxFut<'a, TodoRepository> {
        Box::pin(self.with_access(tenant, move |access| async move {
            Ok(TodoRepository {
                full_name: access.repository.full_name.clone(),
                url: access.repository.html_url.clone(),
            })
        }))
    }
}
