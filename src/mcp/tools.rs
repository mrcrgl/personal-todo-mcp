//! Tool definitions. Note what is absent from every argument type: an owner or
//! repository. Both follow from the caller's token.

use axum::http::request::Parts;
use rmcp::{
    ErrorData as McpError,
    handler::server::{tool::Extension, wrapper::Parameters},
    model::CallToolResult,
    tool, tool_router,
};
use serde_json::json;

use crate::domain::todo::{TodoId, TodoState};
use crate::mcp::dto::{
    CommentArgs, CommentDto, CreateArgs, IdArgs, LabelDeleteArgs, LabelsDto, ListArgs,
    TodoDetailDto, TodoDto, TodoPageDto, UpdateArgs,
};
use crate::mcp::server::{TodoMcp, invalid, tenant, tool_result};

#[tool_router(router = todo_router, vis = "pub")]
impl TodoMcp {
    #[tool(
        name = "todo_list",
        description = "List todos, soonest due first, then by priority. Shows open todos unless a state is given. Filter by priority, list, tag, or due date.",
        annotations(read_only_hint = true)
    )]
    pub async fn todo_list(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<ListArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        let filter = match args.into_filter() {
            Ok(filter) => filter,
            Err(error) => return Ok(invalid(error)),
        };
        tool_result(
            self.todos
                .list(tenant, &filter)
                .await
                .map(TodoPageDto::from),
        )
    }

    #[tool(
        name = "todo_get",
        description = "Read one todo in full, including its notes and comments.",
        annotations(read_only_hint = true)
    )]
    pub async fn todo_get(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<IdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        tool_result(
            self.todos
                .get(tenant, TodoId(args.id))
                .await
                .map(|detail| TodoDetailDto {
                    todo: detail.todo.into(),
                    comments: detail.comments.into_iter().map(Into::into).collect(),
                }),
        )
    }

    #[tool(name = "todo_create", description = "Create a todo.")]
    pub async fn todo_create(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<CreateArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        let todo = match args.into_new_todo() {
            Ok(todo) => todo,
            Err(error) => return Ok(invalid(error)),
        };
        tool_result(self.todos.create(tenant, &todo).await.map(TodoDto::from))
    }

    #[tool(
        name = "todo_update",
        description = "Change a todo. Only the given fields change; use clear to empty a field."
    )]
    pub async fn todo_update(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<UpdateArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        let id = TodoId(args.id);
        let patch = match args.into_patch() {
            Ok(patch) => patch,
            Err(error) => return Ok(invalid(error)),
        };
        tool_result(
            self.todos
                .update(tenant, id, patch)
                .await
                .map(TodoDto::from),
        )
    }

    #[tool(name = "todo_complete", description = "Mark a todo as done.")]
    pub async fn todo_complete(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<IdArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.transition(&parts, args.id, TodoState::Done).await
    }

    #[tool(
        name = "todo_drop",
        description = "Close a todo that will not be done. It stays in the history."
    )]
    pub async fn todo_drop(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<IdArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.transition(&parts, args.id, TodoState::Dropped).await
    }

    #[tool(name = "todo_reopen", description = "Reopen a done or dropped todo.")]
    pub async fn todo_reopen(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<IdArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.transition(&parts, args.id, TodoState::Open).await
    }

    #[tool(name = "todo_comment", description = "Add a comment to a todo.")]
    pub async fn todo_comment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<CommentArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        tool_result(
            self.todos
                .comment(tenant, TodoId(args.id), &args.body)
                .await
                .map(CommentDto::from),
        )
    }

    #[tool(
        name = "labels_list",
        description = "List the lists and tags in use.",
        annotations(read_only_hint = true)
    )]
    pub async fn labels_list(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        tool_result(self.todos.labels(tenant).await.map(LabelsDto::from))
    }

    #[tool(
        name = "label_delete",
        description = "Delete a list or tag. Its todos stay, but lose that list or tag.",
        annotations(destructive_hint = true)
    )]
    pub async fn label_delete(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<LabelDeleteArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        let label = match args.into_label() {
            Ok(label) => label,
            Err(error) => return Ok(invalid(error)),
        };
        tool_result(
            self.todos
                .delete_label(tenant, &label)
                .await
                .map(|()| json!({"deleted": true})),
        )
    }

    #[tool(
        name = "todo_repository",
        description = "Show which GitHub repository the todos are stored in.",
        annotations(read_only_hint = true)
    )]
    pub async fn todo_repository(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        tool_result(
            self.todos
                .repository(tenant)
                .await
                .map(|repo| json!({"repository": repo.full_name, "url": repo.url})),
        )
    }
}

impl TodoMcp {
    async fn transition(
        &self,
        parts: &Parts,
        id: u64,
        state: TodoState,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(parts)?;
        tool_result(
            self.todos
                .transition(tenant, TodoId(id), state)
                .await
                .map(TodoDto::from),
        )
    }
}
