//! Identity port: what the authorization server needs from the upstream
//! identity provider.

use std::future::Future;
use std::pin::Pin;

use crate::domain::tenant::{RepositoryProblem, Tenant};

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = Result<T, IdentityError>> + Send + 'a>>;

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    /// The code or refresh token is invalid, expired, or already used.
    #[error("the grant was rejected")]
    Rejected,
    #[error("{0}")]
    Repository(RepositoryProblem),
    #[error("identity provider unavailable: {0}")]
    Unavailable(String),
}

/// Upstream user tokens. Hence no `Debug`.
pub struct GrantedTokens {
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityUser {
    pub id: u64,
    pub login: String,
}

pub trait IdentityProvider: Send + Sync {
    fn authorize_url(&self, state: &str, redirect_uri: &str) -> String;
    fn install_url(&self) -> String;
    fn installation_settings_url(&self, installation_id: u64) -> String;
    fn exchange_code<'a>(
        &'a self,
        code: &'a str,
        redirect_uri: &'a str,
    ) -> BoxFut<'a, GrantedTokens>;
    /// Single-use: the given token stops working when this succeeds.
    fn refresh<'a>(&'a self, refresh_token: &'a str) -> BoxFut<'a, GrantedTokens>;
    fn user<'a>(&'a self, access_token: &'a str) -> BoxFut<'a, IdentityUser>;
    /// The installation on the user's own account, verified with their token.
    fn personal_installation<'a>(
        &'a self,
        access_token: &'a str,
        user_id: u64,
    ) -> BoxFut<'a, Option<u64>>;
    fn check_repository<'a>(&'a self, tenant: &'a Tenant) -> BoxFut<'a, ()>;
}
