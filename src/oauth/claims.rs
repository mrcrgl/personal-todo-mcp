//! What each sealed value contains. These types are the server's state: they
//! travel with the client, the browser, or GitHub instead of living in a store.

use serde::{Deserialize, Serialize};

use crate::domain::tenant::Tenant;

/// `client_id`: the registration itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientClaims {
    pub redirect_uris: Vec<String>,
    pub client_name: Option<String>,
}

/// A validated authorization request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizationRequest {
    /// Short stand-in for the client id, which would otherwise bloat every
    /// URL that carries this request.
    pub client_fingerprint: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub state: Option<String>,
}

/// Issued with the consent page; valid only in the browser holding `nonce`.
#[derive(Serialize, Deserialize)]
pub struct ConsentClaims {
    pub request: AuthorizationRequest,
    pub nonce: String,
}

/// `state` sent to GitHub for the sign-in.
#[derive(Serialize, Deserialize)]
pub struct FlowClaims {
    pub request: AuthorizationRequest,
    pub nonce: String,
}

/// Kept in a browser cookie while the user installs the App.
#[derive(Serialize, Deserialize)]
pub struct InstallClaims {
    pub request: AuthorizationRequest,
    pub user_id: u64,
    pub login: String,
    pub github_access_token: String,
    pub github_refresh_token: String,
}

#[derive(Serialize, Deserialize)]
pub struct CodeClaims {
    pub client_fingerprint: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub tenant: Tenant,
    pub github_refresh_token: String,
}

#[derive(Serialize, Deserialize)]
pub struct AccessClaims {
    pub tenant: Tenant,
    /// RFC 8707 audience: the MCP endpoint this token is valid for.
    pub audience: String,
}

#[derive(Serialize, Deserialize)]
pub struct RefreshClaims {
    pub client_fingerprint: String,
    pub tenant: Tenant,
    pub github_refresh_token: String,
}
