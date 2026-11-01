//! The authorization flow. Handlers stay thin; every protocol decision is here.
//!
//! Browser path: `/authorize` → consent → GitHub sign-in → (App installation
//! when there is none) → back to the client with a code. Sign-in comes first
//! because GitHub's install page does not return to the caller when the App is
//! already installed, which would strand returning users.

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::domain::tenant::Tenant;
use crate::oauth::claims::{
    AccessClaims, AuthorizationRequest, ClientClaims, CodeClaims, ConsentClaims, FlowClaims,
    InstallClaims, RefreshClaims,
};
use crate::oauth::dto::{
    AuthorizationServerMetadata, AuthorizeParams, GitHubCallbackParams, RegistrationRequest,
    RegistrationResponse, ResourceMetadata, TokenRequest, TokenResponse,
};
use crate::oauth::identity::{IdentityError, IdentityProvider, IdentityUser};
use crate::oauth::seal::{Purpose, SealError, Sealer, random_nonce};
use crate::oauth::{pkce, redirect};

pub const SCOPE: &str = "todos";
pub const FLOW_TTL: u64 = 30 * 60;
const CONSENT_TTL: u64 = 10 * 60;
const CODE_TTL: u64 = 60;
const ACCESS_TTL: u64 = 60 * 60;
/// Just under the six months GitHub's refresh tokens live.
const REFRESH_TTL: u64 = 175 * 24 * 60 * 60;

const MAX_REDIRECT_URIS: usize = 5;
const MAX_REDIRECT_URI_CHARS: usize = 512;
const MAX_CLIENT_NAME_CHARS: usize = 100;
const MAX_STATE_CHARS: usize = 1024;

const SETUP_PATH: &str = "/oauth/github/setup";
const CALLBACK_PATH: &str = "/oauth/github/callback";

#[derive(Debug, thiserror::Error)]
pub enum RegistrationError {
    #[error("{0}")]
    InvalidRedirectUri(String),
    #[error("{0}")]
    InvalidMetadata(String),
    #[error("registration could not be completed")]
    Internal,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthorizeError {
    /// The client is unverified, so the error must not leave this server.
    #[error("{0}")]
    Page(String),
    #[error("redirecting an error to the client")]
    Redirect(String),
}

#[derive(Debug, thiserror::Error)]
pub enum FlowError {
    #[error("{0}")]
    Rejected(String),
    #[error("GitHub could not be reached: try again in a moment")]
    Unavailable,
}

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("{0}")]
    InvalidRequest(String),
    #[error("{0}")]
    InvalidGrant(String),
    #[error("{0}")]
    InvalidTarget(String),
    #[error("unsupported grant type")]
    UnsupportedGrantType,
    #[error("try again later")]
    TemporarilyUnavailable,
    #[error("the server could not issue a token")]
    Internal,
}

pub struct ConsentView {
    pub client_name: Option<String>,
    pub target: String,
    pub consent_token: String,
    pub nonce: String,
}

/// Where an authorization flow goes next.
pub enum FlowOutcome {
    Complete {
        redirect: String,
    },
    Install {
        url: String,
        install_state: String,
    },
    Blocked {
        message: String,
        action_url: String,
        action_label: &'static str,
        retry_url: String,
        install_state: String,
    },
}

pub struct OAuthService {
    sealer: Sealer,
    identity: Arc<dyn IdentityProvider>,
    base_url: String,
    clock: fn() -> u64,
}

impl OAuthService {
    pub fn new(
        sealer: Sealer,
        identity: Arc<dyn IdentityProvider>,
        base_url: String,
        clock: fn() -> u64,
    ) -> Self {
        Self {
            sealer,
            identity,
            base_url,
            clock,
        }
    }

    /// The RFC 8707 resource identifier of the MCP endpoint.
    pub fn resource(&self) -> String {
        format!("{}/mcp", self.base_url)
    }

    pub fn resource_metadata_url(&self) -> String {
        format!("{}/.well-known/oauth-protected-resource/mcp", self.base_url)
    }

    pub fn resource_metadata(&self) -> ResourceMetadata {
        ResourceMetadata {
            resource: self.resource(),
            authorization_servers: vec![self.base_url.clone()],
            scopes_supported: [SCOPE],
            bearer_methods_supported: ["header"],
        }
    }

    pub fn server_metadata(&self) -> AuthorizationServerMetadata {
        AuthorizationServerMetadata {
            issuer: self.base_url.clone(),
            authorization_endpoint: format!("{}/authorize", self.base_url),
            token_endpoint: format!("{}/token", self.base_url),
            registration_endpoint: format!("{}/register", self.base_url),
            scopes_supported: [SCOPE],
            response_types_supported: ["code"],
            grant_types_supported: ["authorization_code", "refresh_token"],
            code_challenge_methods_supported: ["S256"],
            token_endpoint_auth_methods_supported: ["none"],
            authorization_response_iss_parameter_supported: true,
        }
    }

    /// Dynamic client registration (RFC 7591). The `client_id` is the sealed
    /// registration, so an open endpoint has nothing to fill up.
    pub fn register(
        &self,
        request: RegistrationRequest,
    ) -> Result<RegistrationResponse, RegistrationError> {
        validate_redirect_uris(&request.redirect_uris)?;
        if let Some(grants) = &request.grant_types
            && !grants.iter().any(|grant| grant == "authorization_code")
        {
            return Err(RegistrationError::InvalidMetadata(
                "grant_types must include authorization_code".into(),
            ));
        }
        if let Some(responses) = &request.response_types
            && !responses.iter().any(|response| response == "code")
        {
            return Err(RegistrationError::InvalidMetadata(
                "response_types must include code".into(),
            ));
        }

        let client_name = request.client_name.as_deref().map(sanitize_client_name);
        let claims = ClientClaims {
            redirect_uris: request.redirect_uris,
            client_name,
        };
        let client_id = self
            .sealer
            .seal(Purpose::Client, &claims, None)
            .map_err(|_| RegistrationError::Internal)?;
        Ok(RegistrationResponse {
            client_id,
            client_id_issued_at: (self.clock)(),
            client_name: claims.client_name,
            redirect_uris: claims.redirect_uris,
            grant_types: ["authorization_code", "refresh_token"],
            response_types: ["code"],
            token_endpoint_auth_method: "none",
        })
    }

    /// Validates an authorization request and prepares the consent page.
    /// `browser_nonce` is the nonce the browser already holds, if any: reusing
    /// it lets two flows run side by side in one browser.
    pub fn begin_authorization(
        &self,
        params: &AuthorizeParams,
        browser_nonce: Option<&str>,
    ) -> Result<ConsentView, AuthorizeError> {
        let client_id = params
            .client_id
            .as_deref()
            .ok_or_else(|| AuthorizeError::Page("client_id is missing".into()))?;
        let client: ClientClaims = self
            .sealer
            .open(Purpose::Client, client_id, self.now())
            .map_err(|_| AuthorizeError::Page("This client is not registered here.".into()))?;
        let redirect_uri = params
            .redirect_uri
            .as_deref()
            .ok_or_else(|| AuthorizeError::Page("redirect_uri is missing".into()))?;
        if !client
            .redirect_uris
            .iter()
            .any(|registered| redirect::matches_registered(registered, redirect_uri))
        {
            return Err(AuthorizeError::Page(
                "This redirect URI was not registered by the client.".into(),
            ));
        }

        // From here on the redirect URI is trusted, so errors go to the client.
        let state = params.state.as_deref();
        let fail = |code: &str, description: &str| {
            AuthorizeError::Redirect(self.client_error(redirect_uri, state, code, description))
        };

        if params.response_type.as_deref() != Some("code") {
            return Err(fail(
                "unsupported_response_type",
                "response_type must be code",
            ));
        }
        let code_challenge = params
            .code_challenge
            .as_deref()
            .filter(|challenge| pkce::is_valid_challenge(challenge))
            .ok_or_else(|| fail("invalid_request", "a valid PKCE code_challenge is required"))?;
        if params.code_challenge_method.as_deref() != Some("S256") {
            return Err(fail(
                "invalid_request",
                "code_challenge_method must be S256",
            ));
        }
        if state.is_some_and(|value| value.chars().count() > MAX_STATE_CHARS) {
            return Err(fail("invalid_request", "state is too long"));
        }
        if !self.resource_matches(params.resource.as_deref()) {
            return Err(fail(
                "invalid_target",
                "this server only serves its own resource",
            ));
        }
        if let Some(scope) = &params.scope
            && scope.split_whitespace().any(|requested| requested != SCOPE)
        {
            return Err(fail("invalid_scope", "only the todos scope exists"));
        }

        let nonce = match browser_nonce {
            Some(existing) => existing.to_string(),
            None => random_nonce().map_err(|_| internal_page())?,
        };
        let request = AuthorizationRequest {
            client_fingerprint: fingerprint(client_id),
            redirect_uri: redirect_uri.to_string(),
            code_challenge: code_challenge.to_string(),
            state: state.map(String::from),
        };
        let consent_token = self
            .sealer
            .seal(
                Purpose::Consent,
                &ConsentClaims {
                    request,
                    nonce: nonce.clone(),
                },
                Some(self.now() + CONSENT_TTL),
            )
            .map_err(|_| internal_page())?;

        Ok(ConsentView {
            client_name: client.client_name,
            target: redirect::display_target(redirect_uri),
            consent_token,
            nonce,
        })
    }

    /// Returns the next URL for the browser. This consent is what stops a
    /// dynamically registered client from riding on a GitHub authorization the
    /// user gave earlier, which GitHub itself would not ask about again.
    pub fn decide_consent(
        &self,
        consent_token: &str,
        browser_nonce: Option<&str>,
        allow: bool,
    ) -> Result<String, FlowError> {
        let consent: ConsentClaims = self
            .sealer
            .open(Purpose::Consent, consent_token, self.now())
            .map_err(|_| expired_flow())?;
        require_same_browser(&consent.nonce, browser_nonce)?;

        if !allow {
            return Ok(self.client_error(
                &consent.request.redirect_uri,
                consent.request.state.as_deref(),
                "access_denied",
                "the user declined",
            ));
        }
        let state = self
            .sealer
            .seal(
                Purpose::Flow,
                &FlowClaims {
                    request: consent.request,
                    nonce: consent.nonce,
                },
                Some(self.now() + FLOW_TTL),
            )
            .map_err(|_| internal_flow())?;
        Ok(self
            .identity
            .authorize_url(&state, &format!("{}{CALLBACK_PATH}", self.base_url)))
    }

    /// GitHub's sign-in redirect.
    pub async fn github_callback(
        &self,
        params: GitHubCallbackParams,
        browser_nonce: Option<&str>,
    ) -> Result<FlowOutcome, FlowError> {
        let state = params.state.as_deref().ok_or_else(expired_flow)?;
        let flow: FlowClaims = self
            .sealer
            .open(Purpose::Flow, state, self.now())
            .map_err(|_| expired_flow())?;
        require_same_browser(&flow.nonce, browser_nonce)?;

        if params.error.is_some() {
            return Ok(FlowOutcome::Complete {
                redirect: self.client_error(
                    &flow.request.redirect_uri,
                    flow.request.state.as_deref(),
                    "access_denied",
                    "GitHub sign-in was not completed",
                ),
            });
        }
        let code = params.code.as_deref().ok_or_else(expired_flow)?;
        let callback_url = format!("{}{CALLBACK_PATH}", self.base_url);
        let tokens = self
            .identity
            .exchange_code(code, &callback_url)
            .await
            .map_err(flow_error)?;
        let user = self
            .identity
            .user(&tokens.access_token)
            .await
            .map_err(flow_error)?;

        self.continue_flow(
            InstallClaims {
                request: flow.request,
                user_id: user.id,
                login: user.login,
                github_access_token: tokens.access_token,
                github_refresh_token: tokens.refresh_token,
            },
            false,
        )
        .await
    }

    /// GitHub's post-installation redirect, or "check again" on the blocked
    /// page. The `installation_id` GitHub passes along is spoofable and not
    /// read at all: the installation is looked up with the user's own token.
    pub async fn installation_setup(&self, install_state: &str) -> Result<FlowOutcome, FlowError> {
        let claims: InstallClaims = self
            .sealer
            .open(Purpose::Install, install_state, self.now())
            .map_err(|_| expired_flow())?;
        self.continue_flow(claims, true).await
    }

    async fn continue_flow(
        &self,
        claims: InstallClaims,
        after_install: bool,
    ) -> Result<FlowOutcome, FlowError> {
        let installation = self
            .identity
            .personal_installation(&claims.github_access_token, claims.user_id)
            .await
            .map_err(flow_error)?;

        let Some(installation_id) = installation else {
            let install_url = self.identity.install_url();
            let install_state = self.seal_install(&claims)?;
            if !after_install {
                return Ok(FlowOutcome::Install {
                    url: install_url,
                    install_state,
                });
            }
            return Ok(FlowOutcome::Blocked {
                message: "No installation was found on your personal GitHub account. \
                          Install the App there, not on an organization."
                    .into(),
                action_url: install_url,
                action_label: "Install the App",
                retry_url: self.setup_url(),
                install_state,
            });
        };

        let tenant = Tenant {
            user_id: claims.user_id,
            login: claims.login.clone(),
            installation_id,
        };
        match self.identity.check_repository(&tenant).await {
            Ok(()) => {}
            Err(IdentityError::Repository(problem)) => {
                return Ok(FlowOutcome::Blocked {
                    message: problem.to_string(),
                    action_url: self.identity.installation_settings_url(installation_id),
                    action_label: "Open App settings",
                    retry_url: self.setup_url(),
                    install_state: self.seal_install(&claims)?,
                });
            }
            Err(other) => return Err(flow_error(other)),
        }

        let code = self
            .sealer
            .seal(
                Purpose::Code,
                &CodeClaims {
                    client_fingerprint: claims.request.client_fingerprint,
                    redirect_uri: claims.request.redirect_uri.clone(),
                    code_challenge: claims.request.code_challenge,
                    tenant,
                    github_refresh_token: claims.github_refresh_token,
                },
                Some(self.now() + CODE_TTL),
            )
            .map_err(|_| internal_flow())?;
        let mut params = vec![("code", code.as_str()), ("iss", self.base_url.as_str())];
        if let Some(state) = &claims.request.state {
            params.push(("state", state));
        }
        let redirect = redirect::with_params(&claims.request.redirect_uri, &params)
            .map_err(|_| internal_flow())?;
        Ok(FlowOutcome::Complete { redirect })
    }

    /// The token endpoint.
    pub async fn token(&self, request: TokenRequest) -> Result<TokenResponse, TokenError> {
        if !self.resource_matches(request.resource.as_deref()) {
            return Err(TokenError::InvalidTarget(
                "this server only serves its own resource".into(),
            ));
        }
        match request.grant_type.as_deref() {
            Some("authorization_code") => self.exchange_code(request),
            Some("refresh_token") => self.refresh(request).await,
            _ => Err(TokenError::UnsupportedGrantType),
        }
    }

    fn exchange_code(&self, request: TokenRequest) -> Result<TokenResponse, TokenError> {
        let code = required(request.code.as_deref(), "code")?;
        let client_id = required(request.client_id.as_deref(), "client_id")?;
        let redirect_uri = required(request.redirect_uri.as_deref(), "redirect_uri")?;
        let verifier = required(request.code_verifier.as_deref(), "code_verifier")?;

        let claims: CodeClaims = self
            .sealer
            .open(Purpose::Code, code, self.now())
            .map_err(|_| invalid_grant("the code is invalid or expired"))?;
        if claims.client_fingerprint != fingerprint(client_id) {
            return Err(invalid_grant("the code was issued to another client"));
        }
        if claims.redirect_uri != redirect_uri {
            return Err(invalid_grant("redirect_uri does not match the code"));
        }
        if !pkce::verify(verifier, &claims.code_challenge) {
            return Err(invalid_grant("PKCE verification failed"));
        }
        tracing::info!(
            user_id = claims.tenant.user_id,
            installation_id = claims.tenant.installation_id,
            "authorization completed"
        );
        self.issue_tokens(
            claims.client_fingerprint,
            claims.tenant,
            claims.github_refresh_token,
        )
    }

    /// Spends the embedded GitHub refresh token, which is what makes this
    /// token single-use without any server state: GitHub invalidates it on
    /// first use, so a replayed copy fails here. It also re-checks that the
    /// user can still reach the installation.
    async fn refresh(&self, request: TokenRequest) -> Result<TokenResponse, TokenError> {
        let refresh_token = required(request.refresh_token.as_deref(), "refresh_token")?;
        let client_id = required(request.client_id.as_deref(), "client_id")?;
        let claims: RefreshClaims = self
            .sealer
            .open(Purpose::Refresh, refresh_token, self.now())
            .map_err(|_| invalid_grant("the refresh token is invalid or expired"))?;
        if claims.client_fingerprint != fingerprint(client_id) {
            return Err(invalid_grant("the refresh token belongs to another client"));
        }

        let tokens = self
            .identity
            .refresh(&claims.github_refresh_token)
            .await
            .map_err(refresh_error)?;
        let user: IdentityUser = self
            .identity
            .user(&tokens.access_token)
            .await
            .map_err(refresh_error)?;
        if user.id != claims.tenant.user_id {
            return Err(invalid_grant("the refresh token belongs to another user"));
        }
        let installation_id = self
            .identity
            .personal_installation(&tokens.access_token, user.id)
            .await
            .map_err(refresh_error)?
            .ok_or_else(|| invalid_grant("the GitHub App is no longer installed"))?;

        self.issue_tokens(
            claims.client_fingerprint,
            Tenant {
                user_id: user.id,
                login: user.login,
                installation_id,
            },
            tokens.refresh_token,
        )
    }

    fn issue_tokens(
        &self,
        client_fingerprint: String,
        tenant: Tenant,
        github_refresh_token: String,
    ) -> Result<TokenResponse, TokenError> {
        let now = self.now();
        let access_token = self
            .sealer
            .seal(
                Purpose::Access,
                &AccessClaims {
                    tenant: tenant.clone(),
                    audience: self.resource(),
                },
                Some(now + ACCESS_TTL),
            )
            .map_err(|_| TokenError::Internal)?;
        let refresh_token = self
            .sealer
            .seal(
                Purpose::Refresh,
                &RefreshClaims {
                    client_fingerprint,
                    tenant,
                    github_refresh_token,
                },
                Some(now + REFRESH_TTL),
            )
            .map_err(|_| TokenError::Internal)?;
        Ok(TokenResponse {
            access_token,
            token_type: "Bearer",
            expires_in: ACCESS_TTL,
            refresh_token,
            scope: SCOPE,
        })
    }

    /// The only way a `Tenant` comes into existence on the MCP path.
    pub fn authenticate(&self, access_token: &str) -> Result<Tenant, SealError> {
        let claims: AccessClaims = self
            .sealer
            .open(Purpose::Access, access_token, self.now())?;
        if claims.audience != self.resource() {
            return Err(SealError::Invalid);
        }
        Ok(claims.tenant)
    }

    /// Missing is accepted because a client without RFC 8707 support can only
    /// mean this server; the token is audience-bound either way.
    fn resource_matches(&self, requested: Option<&str>) -> bool {
        requested.is_none_or(|resource| resource.trim_end_matches('/') == self.resource())
    }

    fn client_error(
        &self,
        redirect_uri: &str,
        state: Option<&str>,
        code: &str,
        description: &str,
    ) -> String {
        let mut params = vec![
            ("error", code),
            ("error_description", description),
            ("iss", self.base_url.as_str()),
        ];
        if let Some(state) = state {
            params.push(("state", state));
        }
        // The URI parsed when it was registered.
        redirect::with_params(redirect_uri, &params).unwrap_or_else(|_| redirect_uri.to_string())
    }

    fn seal_install(&self, claims: &InstallClaims) -> Result<String, FlowError> {
        self.sealer
            .seal(Purpose::Install, claims, Some(self.now() + FLOW_TTL))
            .map_err(|_| internal_flow())
    }

    fn setup_url(&self) -> String {
        format!("{}{SETUP_PATH}", self.base_url)
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }
}

fn validate_redirect_uris(uris: &[String]) -> Result<(), RegistrationError> {
    if uris.is_empty() || uris.len() > MAX_REDIRECT_URIS {
        return Err(RegistrationError::InvalidRedirectUri(format!(
            "between 1 and {MAX_REDIRECT_URIS} redirect_uris are required"
        )));
    }
    for uri in uris {
        if uri.chars().count() > MAX_REDIRECT_URI_CHARS || !redirect::is_registrable(uri) {
            return Err(RegistrationError::InvalidRedirectUri(format!(
                "{uri:?} must be https, loopback http, or a private app scheme"
            )));
        }
    }
    Ok(())
}

fn sanitize_client_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .take(MAX_CLIENT_NAME_CHARS)
        .collect()
}

fn fingerprint(client_id: &str) -> String {
    B64.encode(&Sha256::digest(client_id.as_bytes())[..16])
}

fn require_same_browser(expected: &str, presented: Option<&str>) -> Result<(), FlowError> {
    let same =
        presented.is_some_and(|nonce| bool::from(nonce.as_bytes().ct_eq(expected.as_bytes())));
    if same {
        Ok(())
    } else {
        Err(FlowError::Rejected(
            "This link was not started in this browser.".into(),
        ))
    }
}

fn required<'a>(value: Option<&'a str>, name: &str) -> Result<&'a str, TokenError> {
    value.ok_or_else(|| TokenError::InvalidRequest(format!("{name} is missing")))
}

fn invalid_grant(description: &str) -> TokenError {
    TokenError::InvalidGrant(description.to_string())
}

fn refresh_error(error: IdentityError) -> TokenError {
    match error {
        IdentityError::Rejected => invalid_grant("the grant is no longer valid: sign in again"),
        IdentityError::Repository(problem) => TokenError::InvalidGrant(problem.to_string()),
        IdentityError::Unavailable(reason) => {
            tracing::warn!(%reason, "token refresh could not reach GitHub");
            TokenError::TemporarilyUnavailable
        }
    }
}

fn flow_error(error: IdentityError) -> FlowError {
    match error {
        IdentityError::Rejected => FlowError::Rejected("GitHub rejected the sign-in.".into()),
        IdentityError::Repository(problem) => FlowError::Rejected(problem.to_string()),
        IdentityError::Unavailable(reason) => {
            tracing::warn!(%reason, "authorization flow could not reach GitHub");
            FlowError::Unavailable
        }
    }
}

fn expired_flow() -> FlowError {
    FlowError::Rejected("This sign-in link is invalid or has expired.".into())
}

fn internal_flow() -> FlowError {
    FlowError::Rejected("The sign-in could not be completed.".into())
}

fn internal_page() -> AuthorizeError {
    AuthorizeError::Page("The sign-in could not be started.".into())
}
