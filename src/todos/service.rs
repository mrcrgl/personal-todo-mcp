//! Todo use cases.

use std::sync::Arc;

use crate::domain::tenant::Tenant;
use crate::domain::todo::{
    LabelRef, NewTodo, Todo, TodoComment, TodoFilter, TodoId, TodoLabels, TodoPage, TodoPatch,
    TodoRepository, TodoState,
};
use crate::todos::store::{StoreError, TodoStore};

pub struct TodoDetail {
    pub todo: Todo,
    pub comments: Vec<TodoComment>,
}

pub struct TodoService {
    store: Arc<dyn TodoStore>,
}

impl TodoService {
    pub fn new(store: Arc<dyn TodoStore>) -> Self {
        Self { store }
    }

    pub async fn list(&self, tenant: &Tenant, filter: &TodoFilter) -> Result<TodoPage, StoreError> {
        let mut page = self.store.list(tenant, filter).await?;
        page.todos.sort_by(Todo::agenda_cmp);
        Ok(page)
    }

    pub async fn get(&self, tenant: &Tenant, id: TodoId) -> Result<TodoDetail, StoreError> {
        let (todo, comments) =
            tokio::try_join!(self.store.get(tenant, id), self.store.comments(tenant, id))?;
        Ok(TodoDetail { todo, comments })
    }

    pub async fn create(&self, tenant: &Tenant, todo: &NewTodo) -> Result<Todo, StoreError> {
        self.store.create(tenant, todo).await
    }

    pub async fn update(
        &self,
        tenant: &Tenant,
        id: TodoId,
        patch: TodoPatch,
    ) -> Result<Todo, StoreError> {
        let mut todo = self.store.get(tenant, id).await?;
        todo.apply(patch);
        self.store.save(tenant, &todo).await
    }

    pub async fn transition(
        &self,
        tenant: &Tenant,
        id: TodoId,
        state: TodoState,
    ) -> Result<Todo, StoreError> {
        let mut todo = self.store.get(tenant, id).await?;
        todo.state = state;
        self.store.save(tenant, &todo).await
    }

    pub async fn comment(
        &self,
        tenant: &Tenant,
        id: TodoId,
        body: &str,
    ) -> Result<TodoComment, StoreError> {
        // Resolves the id first: a pull request shares the issue number space
        // and must not be commented on through a todo id.
        self.store.get(tenant, id).await?;
        self.store.add_comment(tenant, id, body).await
    }

    pub async fn labels(&self, tenant: &Tenant) -> Result<TodoLabels, StoreError> {
        self.store.labels(tenant).await
    }

    pub async fn delete_label(&self, tenant: &Tenant, label: &LabelRef) -> Result<(), StoreError> {
        self.store.delete_label(tenant, label).await
    }

    pub async fn repository(&self, tenant: &Tenant) -> Result<TodoRepository, StoreError> {
        self.store.repository(tenant).await
    }
}
