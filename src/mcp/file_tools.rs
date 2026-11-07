//! File tools. As with the todo tools, no argument names an owner or
//! repository: both follow from the caller's token.

use axum::http::request::Parts;
use rmcp::{
    ErrorData as McpError,
    handler::server::{tool::Extension, wrapper::Parameters},
    model::{CallToolResult, ContentBlock},
    tool, tool_router,
};
use serde_json::json;

use crate::files::service::FileError;
use crate::mcp::file_dto::{
    FileDeleteArgs, FileEditArgs, FileListArgs, FileListingDto, FileReadArgs, FileWriteArgs,
    WriteDto, render_edit, render_view,
};
use crate::mcp::server::{TodoMcp, invalid, tenant};

#[tool_router(router = file_router, vis = "pub")]
impl TodoMcp {
    #[tool(
        name = "file_list",
        description = "List the Markdown files in the user's repository, with their size in bytes.",
        annotations(read_only_hint = true)
    )]
    pub async fn file_list(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<FileListArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        let folder = match args.into_folder() {
            Ok(folder) => folder,
            Err(error) => return Ok(invalid(error)),
        };
        let listed = self.files.list(tenant, &folder).await;
        file_result(listed, |listing| {
            serde_json::to_string(&FileListingDto::from(listing))
                .expect("listing serializes to JSON")
        })
    }

    #[tool(
        name = "file_read",
        description = "Read a Markdown file as numbered lines, 400 at a time by default. Use \
                       offset and limit for long files. Returns the sha that file_write and \
                       file_delete need. Line numbers are not part of the file.",
        annotations(read_only_hint = true)
    )]
    pub async fn file_read(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<FileReadArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        let request = match args.into_request() {
            Ok(request) => request,
            Err(error) => return Ok(invalid(error)),
        };
        let read = self
            .files
            .read(tenant, &request.path, request.offset, request.limit)
            .await;
        file_result(read, |view| render_view(&view, request.offset))
    }

    #[tool(
        name = "file_edit",
        description = "Change part of a Markdown file by exact string replacement. Prefer \
                       this over file_write for changes to existing files. Copy old_string \
                       from file_read output without the line numbers. Each old_string must \
                       match exactly once unless replace_all is set. A change by someone \
                       else in the meantime is handled automatically."
    )]
    pub async fn file_edit(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<FileEditArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        let (path, edits) = match args.into_edits() {
            Ok(request) => request,
            Err(error) => return Ok(invalid(error)),
        };
        let edited = self.files.edit(tenant, &path, &edits).await;
        file_result(edited, |outcome| render_edit(&outcome))
    }

    #[tool(
        name = "file_write",
        description = "Create a Markdown file, or replace a whole file. Omit sha to create; \
                       to replace, pass the sha from file_read. Up to 512 KB."
    )]
    pub async fn file_write(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<FileWriteArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        let (path, text, sha) = match args.into_write() {
            Ok(request) => request,
            Err(error) => return Ok(invalid(error)),
        };
        let written = self.files.write(tenant, &path, &text, sha).await;
        file_result(written, |outcome| {
            serde_json::to_string(&WriteDto::from(outcome)).expect("outcome serializes to JSON")
        })
    }

    #[tool(
        name = "file_delete",
        description = "Delete a Markdown file. Needs the sha from file_read. It stays in the \
                       repository's history.",
        annotations(destructive_hint = true)
    )]
    pub async fn file_delete(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<FileDeleteArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tenant = tenant(&parts)?;
        let (path, sha) = match args.into_delete() {
            Ok(request) => request,
            Err(error) => return Ok(invalid(error)),
        };
        let deleted = self.files.delete(tenant, &path, &sha).await;
        file_result(deleted, |()| json!({"deleted": true}).to_string())
    }
}

/// Like `tool_result`, but logs only the error's kind: GitHub's messages for
/// file requests can name the path, and paths count as content.
fn file_result<T>(
    result: Result<T, FileError>,
    render: impl FnOnce(T) -> String,
) -> Result<CallToolResult, McpError> {
    match result {
        Ok(value) => Ok(CallToolResult::success(vec![ContentBlock::text(render(
            value,
        ))])),
        Err(error) => {
            tracing::warn!(kind = error.kind(), "file tool call failed");
            Ok(CallToolResult::error(vec![ContentBlock::text(
                error.to_string(),
            )]))
        }
    }
}
