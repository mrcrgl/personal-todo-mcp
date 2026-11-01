//! Guards the MCP endpoint: a valid access token or a discoverable 401.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::oauth::service::OAuthService;

/// Puts the authenticated `Tenant` into the request extensions, which is where
/// the MCP tools read it from.
pub async fn require_tenant(
    State(oauth): State<Arc<OAuthService>>,
    mut request: Request,
    next: Next,
) -> Response {
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));

    let Some(token) = token else {
        return unauthorized(&oauth, None);
    };
    match oauth.authenticate(token) {
        Ok(tenant) => {
            request.extensions_mut().insert(tenant);
            next.run(request).await
        }
        Err(_) => unauthorized(&oauth, Some("invalid_token")),
    }
}

/// `WWW-Authenticate` is how a client finds the authorization server, so this
/// response is the start of onboarding as much as a rejection (RFC 9728 §5.1).
fn unauthorized(oauth: &OAuthService, error: Option<&str>) -> Response {
    let error = error
        .map(|code| format!(", error=\"{code}\""))
        .unwrap_or_default();
    let challenge = format!(
        "Bearer resource_metadata=\"{}\"{error}",
        oauth.resource_metadata_url()
    );
    let mut response = (StatusCode::UNAUTHORIZED, "unauthorized\n").into_response();
    if let Ok(value) = HeaderValue::from_str(&challenge) {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
    }
    response
}
