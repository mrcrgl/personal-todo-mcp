//! MCP transport layer: tool schemas, argument mapping, and result mapping.
//! No business rules live here; tools call `TodoService` and `FileService`.

pub mod dto;
pub mod file_dto;
pub mod file_tools;
pub mod server;
pub mod tools;
