//! Table tools. As with the other tools, no argument names an owner or
//! repository: both follow from the caller's token.

use axum::http::request::Parts;
use rmcp::{
    ErrorData as McpError,
    handler::server::{tool::Extension, wrapper::Parameters},
    model::{CallToolResult, ContentBlock},
    tool, tool_router,
};
use serde::Serialize;
use serde_json::json;

use crate::mcp::server::{TodoMcp, invalid, tenant};
use crate::mcp::table_dto::{
    InsertDto, QueryDto, TableAlterArgs, TableArgs, TableCreateArgs, TableDeleteArgs,
    TableDropArgs, TableDto, TableInsertArgs, TableListingDto, TableQueryArgs, TableUpdateArgs,
    affected_json,
};
use crate::tables::service::TableServiceError;

#[tool_router(router = table_router, vis = "pub")]
impl TodoMcp {
    #[tool(
        name = "table_list",
        description = "List the user's tables, with their size in bytes.",
        annotations(read_only_hint = true)
    )]
    pub async fn table_list(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        let listed = self.tables.list(tenant).await;
        table_result(listed.map(TableListingDto::from))
    }

    #[tool(
        name = "table_create",
        description = "Create a table: a list of structured rows with a typed schema, e.g. a \
                       reading list or an inventory. Column types: string, integer, number, \
                       boolean, date (YYYY-MM-DD), datetime (RFC 3339), enum (give values), \
                       list (of strings). Every row gets an id from the server. Check \
                       table_list first to reuse an existing table."
    )]
    pub async fn table_create(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<TableCreateArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        let request = match args.into_request() {
            Ok(request) => request,
            Err(error) => return Ok(invalid(error)),
        };
        let created = self
            .tables
            .create(tenant, &request.name, request.schema, request.description)
            .await;
        table_result(created.map(|stored| TableDto::new(&request.name, &stored.table, stored.sha)))
    }

    #[tool(
        name = "table_describe",
        description = "Show a table's columns, row count, and the sha that table_drop needs.",
        annotations(read_only_hint = true)
    )]
    pub async fn table_describe(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<TableArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        let name = match args.into_name() {
            Ok(name) => name,
            Err(error) => return Ok(invalid(error)),
        };
        let described = self.tables.describe(tenant, &name).await;
        table_result(described.map(|stored| TableDto::new(&name, &stored.table, stored.sha)))
    }

    #[tool(
        name = "table_alter",
        description = "Add or remove columns. Removals apply first, so a column can be \
                       replaced in one call. A required column added to a table with rows \
                       needs a default. Removing a column deletes its values."
    )]
    pub async fn table_alter(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<TableAlterArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        let (name, change) = match args.into_change() {
            Ok(request) => request,
            Err(error) => return Ok(invalid(error)),
        };
        let altered = self.tables.change(tenant, &name, &change).await;
        table_result(altered.map(|outcome| TableDto::new(&name, &outcome.table, outcome.sha)))
    }

    #[tool(
        name = "table_drop",
        description = "Delete a whole table with all its rows. Needs the sha from \
                       table_describe or table_query. It stays in the repository's history.",
        annotations(destructive_hint = true)
    )]
    pub async fn table_drop(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<TableDropArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        let (name, sha) = match args.into_drop() {
            Ok(request) => request,
            Err(error) => return Ok(invalid(error)),
        };
        let dropped = self.tables.drop_table(tenant, &name, &sha).await;
        table_result(dropped.map(|()| json!({"dropped": true})))
    }

    #[tool(
        name = "table_query",
        description = "Read rows of a table, 100 at a time by default. Filter with where \
                       (conditions on columns or id, all of which must hold), sort with \
                       order_by, and page with offset and limit.",
        annotations(read_only_hint = true)
    )]
    pub async fn table_query(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<TableQueryArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        let (name, query) = match args.into_query() {
            Ok(request) => request,
            Err(error) => return Ok(invalid(error)),
        };
        let queried = self.tables.query(tenant, &name, &query).await;
        table_result(queried.map(QueryDto::from))
    }

    #[tool(
        name = "table_insert",
        description = "Add up to 500 rows in one call; prefer one call for many rows. Each \
                       row is an object of column to value. Returns the new ids. Either all \
                       rows are added or none."
    )]
    pub async fn table_insert(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<TableInsertArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        let (name, change) = match args.into_change() {
            Ok(request) => request,
            Err(error) => return Ok(invalid(error)),
        };
        let inserted = self.tables.change(tenant, &name, &change).await;
        table_result(inserted.map(InsertDto::from))
    }

    #[tool(
        name = "table_update",
        description = "Change rows: either up to 500 rows by id, each with its own set, or \
                       every row matching where with one set. null empties a cell. Either \
                       all changes apply or none."
    )]
    pub async fn table_update(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<TableUpdateArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        let (name, change) = match args.into_change() {
            Ok(request) => request,
            Err(error) => return Ok(invalid(error)),
        };
        let updated = self.tables.change(tenant, &name, &change).await;
        table_result(updated.map(|outcome| affected_json(&outcome, "updated")))
    }

    #[tool(
        name = "table_delete",
        description = "Delete rows by id (up to 500) or every row matching where. Rows stay \
                       in the repository's history.",
        annotations(destructive_hint = true)
    )]
    pub async fn table_delete(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<TableDeleteArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        let (name, change) = match args.into_change() {
            Ok(request) => request,
            Err(error) => return Ok(invalid(error)),
        };
        let deleted = self.tables.change(tenant, &name, &change).await;
        table_result(deleted.map(|outcome| affected_json(&outcome, "deleted")))
    }
}

/// Logs only the error's kind: table names and GitHub's messages, which can
/// carry the path, count as content.
fn table_result<T: Serialize>(
    result: Result<T, TableServiceError>,
) -> Result<CallToolResult, McpError> {
    match result {
        Ok(value) => {
            let text = serde_json::to_string(&value)
                .map_err(|e| McpError::internal_error(format!("serialize response: {e}"), None))?;
            Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
        }
        Err(error) => {
            tracing::warn!(kind = error.kind(), "table tool call failed");
            Ok(CallToolResult::error(vec![ContentBlock::text(
                error.to_string(),
            )]))
        }
    }
}
