//! Storage port. Every operation takes the tenant it acts for, so an
//! implementation cannot be called without one.

use std::future::Future;
use std::pin::Pin;

use crate::domain::tenant::{RepositoryProblem, Tenant};
use crate::domain::todo::{
    LabelRef, NewTodo, Todo, TodoComment, TodoFilter, TodoId, TodoLabels, TodoPage, TodoRepository,
};

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = Result<T, StoreError>> + Send + 'a>>;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("todo #{0} does not exist")]
    NotFound(u64),
    #[error("the GitHub App is no longer installed: reconnect to install it again")]
    InstallationGone,
    #[error("{0}")]
    Repository(RepositoryProblem),
    #[error(
        "the GitHub App lacks a permission this tool needs: approve its updated permissions \
         in your GitHub settings under Applications → Installed GitHub Apps, then try again"
    )]
    PermissionNotGranted,
    #[error("GitHub rate limit reached: try again later")]
    RateLimited,
    /// GitHub refused the request; `message` is GitHub's own and safe to show.
    #[error("GitHub rejected the request ({status}): {message}")]
    Rejected { status: u16, message: String },
    #[error("GitHub could not be reached: {0}")]
    Unavailable(String),
}

pub trait TodoStore: Send + Sync {
    fn list<'a>(&'a self, tenant: &'a Tenant, filter: &'a TodoFilter) -> BoxFut<'a, TodoPage>;
    fn get<'a>(&'a self, tenant: &'a Tenant, id: TodoId) -> BoxFut<'a, Todo>;
    fn comments<'a>(&'a self, tenant: &'a Tenant, id: TodoId) -> BoxFut<'a, Vec<TodoComment>>;
    fn create<'a>(&'a self, tenant: &'a Tenant, todo: &'a NewTodo) -> BoxFut<'a, Todo>;
    /// Writes title, notes, state, and labels of an existing todo.
    fn save<'a>(&'a self, tenant: &'a Tenant, todo: &'a Todo) -> BoxFut<'a, Todo>;
    fn add_comment<'a>(
        &'a self,
        tenant: &'a Tenant,
        id: TodoId,
        body: &'a str,
    ) -> BoxFut<'a, TodoComment>;
    fn labels<'a>(&'a self, tenant: &'a Tenant) -> BoxFut<'a, TodoLabels>;
    fn delete_label<'a>(&'a self, tenant: &'a Tenant, label: &'a LabelRef) -> BoxFut<'a, ()>;
    fn repository<'a>(&'a self, tenant: &'a Tenant) -> BoxFut<'a, TodoRepository>;
}
