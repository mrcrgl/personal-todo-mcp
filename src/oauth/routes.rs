//! HTTP surface of the authorization server. Handlers parse, call
//! `OAuthService`, and map the outcome.

use std::sync::Arc;

use axum::{
    Form, Json, Router,
    extract::{DefaultBodyLimit, Query, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use tower_http::cors::{Any, CorsLayer};

use crate::oauth::dto::{
    AuthorizeParams, ConsentForm, GitHubCallbackParams, OAuthErrorBody, RegistrationRequest,
    TokenRequest,
};
use crate::oauth::pages;
use crate::oauth::service::{
    AuthorizeError, FLOW_TTL, FlowError, FlowOutcome, OAuthService, RegistrationError, TokenError,
};

const FLOW_COOKIE: &str = "__Host-todo_flow";
const INSTALL_COOKIE: &str = "__Host-todo_install";
const MAX_BODY_BYTES: usize = 16 * 1024;
/// `form-action` is left out on purpose: browsers apply it to the redirects
/// after a form post, which here lead to GitHub or to the MCP client.
const PAGE_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'";
const MCP_SESSION_HEADER: HeaderName = HeaderName::from_static("mcp-session-id");

pub fn router(oauth: Arc<OAuthService>) -> Router {
    let api = Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(resource_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(server_metadata),
        )
        .route("/register", post(register))
        .route("/token", post(token))
        .layer(cors());
    let browser = Router::new()
        .route("/authorize", get(authorize))
        .route("/authorize/consent", post(consent))
        .route("/oauth/github/callback", get(github_callback))
        .route("/oauth/github/setup", get(github_setup));

    api.merge(browser)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(oauth)
}

/// For the bearer-authenticated endpoints only: they use no cookies, so any
/// origin may call them, which browser-based MCP clients need.
pub fn cors() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
        .expose_headers([header::WWW_AUTHENTICATE, MCP_SESSION_HEADER])
}

async fn resource_metadata(State(oauth): State<Arc<OAuthService>>) -> Response {
    Json(oauth.resource_metadata()).into_response()
}

async fn server_metadata(State(oauth): State<Arc<OAuthService>>) -> Response {
    Json(oauth.server_metadata()).into_response()
}

async fn register(
    State(oauth): State<Arc<OAuthService>>,
    body: Result<Json<RegistrationRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = body else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            "the body must be a JSON client registration".into(),
        );
    };
    match oauth.register(request) {
        Ok(response) => (StatusCode::CREATED, no_store(), Json(response)).into_response(),
        Err(error) => {
            let code = match error {
                RegistrationError::InvalidRedirectUri(_) => "invalid_redirect_uri",
                RegistrationError::InvalidMetadata(_) => "invalid_client_metadata",
                RegistrationError::Internal => "server_error",
            };
            oauth_error(StatusCode::BAD_REQUEST, code, error.to_string())
        }
    }
}

async fn token(
    State(oauth): State<Arc<OAuthService>>,
    Form(request): Form<TokenRequest>,
) -> Response {
    match oauth.token(request).await {
        Ok(response) => (no_store(), Json(response)).into_response(),
        Err(error) => {
            let (status, code) = match error {
                TokenError::InvalidRequest(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
                TokenError::InvalidGrant(_) => (StatusCode::BAD_REQUEST, "invalid_grant"),
                TokenError::InvalidTarget(_) => (StatusCode::BAD_REQUEST, "invalid_target"),
                TokenError::UnsupportedGrantType => {
                    (StatusCode::BAD_REQUEST, "unsupported_grant_type")
                }
                TokenError::TemporarilyUnavailable => {
                    (StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable")
                }
                TokenError::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
            };
            oauth_error(status, code, error.to_string())
        }
    }
}

async fn authorize(
    State(oauth): State<Arc<OAuthService>>,
    headers: HeaderMap,
    Query(params): Query<AuthorizeParams>,
) -> Response {
    match oauth.begin_authorization(&params, cookie(&headers, FLOW_COOKIE)) {
        Ok(view) => {
            let page = pages::consent(
                view.client_name.as_deref(),
                &view.target,
                &view.consent_token,
            );
            with_cookies(
                html(StatusCode::OK, page),
                &[set_cookie(FLOW_COOKIE, &view.nonce, FLOW_TTL)],
            )
        }
        Err(AuthorizeError::Page(message)) => {
            html(StatusCode::BAD_REQUEST, pages::problem(&message))
        }
        Err(AuthorizeError::Redirect(url)) => Redirect::to(&url).into_response(),
    }
}

async fn consent(
    State(oauth): State<Arc<OAuthService>>,
    headers: HeaderMap,
    Form(form): Form<ConsentForm>,
) -> Response {
    let allow = form.decision == "allow";
    match oauth.decide_consent(&form.consent, cookie(&headers, FLOW_COOKIE), allow) {
        Ok(url) => Redirect::to(&url).into_response(),
        Err(error) => flow_problem(error),
    }
}

async fn github_callback(
    State(oauth): State<Arc<OAuthService>>,
    headers: HeaderMap,
    Query(params): Query<GitHubCallbackParams>,
) -> Response {
    match oauth
        .github_callback(params, cookie(&headers, FLOW_COOKIE))
        .await
    {
        Ok(outcome) => flow_response(outcome),
        Err(error) => flow_problem(error),
    }
}

/// Also reached by people who install the App from its GitHub page without
/// coming from a client; those have no flow to resume.
async fn github_setup(State(oauth): State<Arc<OAuthService>>, headers: HeaderMap) -> Response {
    let Some(install_state) = cookie(&headers, INSTALL_COOKIE) else {
        return html(StatusCode::OK, pages::installed());
    };
    match oauth.installation_setup(install_state).await {
        Ok(outcome) => flow_response(outcome),
        Err(error) => flow_problem(error),
    }
}

fn flow_response(outcome: FlowOutcome) -> Response {
    match outcome {
        FlowOutcome::Complete { redirect } => with_cookies(
            Redirect::to(&redirect).into_response(),
            // Only this one: it holds GitHub tokens, while the flow nonce may
            // still be serving a parallel flow in this browser.
            &[clear_cookie(INSTALL_COOKIE)],
        ),
        FlowOutcome::Install { url, install_state } => with_cookies(
            Redirect::to(&url).into_response(),
            &[set_cookie(INSTALL_COOKIE, &install_state, FLOW_TTL)],
        ),
        FlowOutcome::Blocked {
            message,
            action_url,
            action_label,
            retry_url,
            install_state,
        } => with_cookies(
            html(
                StatusCode::OK,
                pages::blocked(&message, &action_url, action_label, &retry_url),
            ),
            &[set_cookie(INSTALL_COOKIE, &install_state, FLOW_TTL)],
        ),
    }
}

fn flow_problem(error: FlowError) -> Response {
    let status = match error {
        FlowError::Rejected(_) => StatusCode::BAD_REQUEST,
        FlowError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    };
    html(status, pages::problem(&error.to_string()))
}

fn html(status: StatusCode, body: String) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::CONTENT_SECURITY_POLICY, PAGE_CSP),
            (header::X_FRAME_OPTIONS, "DENY"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        body,
    )
        .into_response()
}

fn no_store() -> [(HeaderName, &'static str); 1] {
    [(header::CACHE_CONTROL, "no-store")]
}

fn oauth_error(status: StatusCode, error: &'static str, error_description: String) -> Response {
    (
        status,
        no_store(),
        Json(OAuthErrorBody {
            error,
            error_description,
        }),
    )
        .into_response()
}

/// The `__Host-` prefix makes browsers refuse a cookie planted from a subdomain
/// or over plain HTTP, which is what lets a cookie prove "this browser".
fn set_cookie(name: &str, value: &str, max_age: u64) -> String {
    format!("{name}={value}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age={max_age}")
}

fn clear_cookie(name: &str) -> String {
    set_cookie(name, "", 0)
}

fn with_cookies(mut response: Response, cookies: &[String]) -> Response {
    for cookie in cookies {
        if let Ok(value) = HeaderValue::from_str(cookie) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, value)| *key == name && !value.is_empty())
        .map(|(_, value)| value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(value: &str) -> HeaderMap {
        let mut map = HeaderMap::new();
        map.insert(header::COOKIE, HeaderValue::from_str(value).unwrap());
        map
    }

    #[test]
    fn finds_a_cookie_among_others() {
        let map = headers("a=1; __Host-todo_flow=abc=; b=2");
        assert_eq!(cookie(&map, FLOW_COOKIE), Some("abc="));
        assert_eq!(cookie(&map, INSTALL_COOKIE), None);
    }

    #[test]
    fn a_cleared_cookie_counts_as_absent() {
        assert_eq!(cookie(&headers("__Host-todo_flow="), FLOW_COOKIE), None);
    }

    #[test]
    fn flow_cookies_are_locked_down() {
        let value = set_cookie(FLOW_COOKIE, "x", 60);
        for attribute in ["Path=/", "Secure", "HttpOnly", "SameSite=Lax"] {
            assert!(value.contains(attribute), "{attribute}");
        }
    }
}
