//! Stateless OAuth 2.1 authorization server for MCP clients.
//!
//! Nothing is stored. Client ids, authorization codes, and tokens are values
//! sealed by `seal`, so any replica can validate them. Refresh tokens carry the
//! user's GitHub refresh token, and GitHub's single-use rotation of that token
//! is what makes a replayed refresh token fail.

pub mod bearer;
pub mod claims;
pub mod dto;
pub mod identity;
pub mod pages;
pub mod pkce;
pub mod redirect;
pub mod routes;
pub mod seal;
pub mod service;
