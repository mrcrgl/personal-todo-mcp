//! Multi-tenant MCP server that manages personal todos as GitHub Issues.
//!
//! Dependencies point one way: `mcp` and `oauth` (transport) call into
//! `todos` and `files` (application), which depend on `domain` and their
//! storage ports. `github` implements the ports; nothing outside `app` and
//! `main` knows which adapter is wired in.

pub mod app;
pub mod config;
pub mod domain;
pub mod files;
pub mod github;
pub mod mcp;
pub mod oauth;
pub mod todos;
