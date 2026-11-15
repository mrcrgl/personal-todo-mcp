//! MCP transport layer: tool schemas, argument mapping, and result mapping.
//! No business rules live here; tools call `TodoService`, `FileService`, and
//! `TableService`.

pub mod dto;
pub mod file_dto;
pub mod file_tools;
pub mod server;
pub mod table_dto;
pub mod table_tools;
pub mod tools;
