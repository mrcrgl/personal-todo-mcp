//! Harness: the real application against a fake GitHub, driven over HTTP the
//! way an MCP client and a browser would.

// Each test binary compiles this module and uses a different part of it.
#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use personal_todo_mcp::app;
use personal_todo_mcp::config::Config;
use reqwest::redirect::Policy;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use url::Url;
use wiremock::matchers::{body_string_contains, header, method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub const TEST_KEY_PEM: &str = include_str!("../fixtures/test-rsa.pem");
pub const CLIENT_REDIRECT: &str = "https://client.example/callback";
pub const PUBLIC_HOST: &str = "todo.example.test";
pub const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";

/// One person as GitHub sees them.
#[derive(Clone)]
pub struct FakeUser {
    pub id: u64,
    pub login: &'static str,
    pub installation_id: u64,
    pub code: &'static str,
}

pub const ALICE: FakeUser = FakeUser {
    id: 1001,
    login: "alice",
    installation_id: 5001,
    code: "code-alice",
};
pub const BOB: FakeUser = FakeUser {
    id: 2002,
    login: "bob",
    installation_id: 5002,
    code: "code-bob",
};

impl FakeUser {
    pub fn user_token(&self) -> String {
        format!("ghu_{}", self.login)
    }
    pub fn refresh_token(&self) -> String {
        format!("ghr_{}", self.login)
    }
    pub fn installation_token(&self) -> String {
        format!("ghs_{}", self.login)
    }
    fn auth(&self, token: String) -> (&'static str, String) {
        ("authorization", format!("Bearer {token}"))
    }
}

pub struct Harness {
    pub base_url: String,
    pub github: MockServer,
    pub http: reqwest::Client,
    _shutdown: CancellationToken,
}

impl Harness {
    pub async fn start() -> Self {
        let github = MockServer::start().await;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let base_url = format!("http://localhost:{}", addr.port());

        let settings = HashMap::from([
            ("PUBLIC_BASE_URL", base_url.clone()),
            ("GH_APP_ID", "12345".to_string()),
            ("GH_APP_SLUG", "personal-todos".to_string()),
            ("GH_APP_CLIENT_ID", "Iv1.test".to_string()),
            ("GH_APP_CLIENT_SECRET", "test-secret".to_string()),
            ("GH_APP_PRIVATE_KEY_PATH", "unused".to_string()),
            ("GH_API_URL", github.uri()),
            ("GH_WEB_URL", github.uri()),
            ("TOKEN_KEYS", B64.encode([7u8; 32])),
        ]);
        let mut cfg = Config::from_lookup(&|k| settings.get(k).cloned()).unwrap();
        // Stands in for a public hostname, which `PUBLIC_BASE_URL` can't be
        // here because the tests talk plain HTTP to a local port.
        cfg.allowed_hosts.push(PUBLIC_HOST.to_string());
        let services = app::build_services(&cfg, TEST_KEY_PEM).unwrap();

        let shutdown = CancellationToken::new();
        let router = app::router(&cfg, services, shutdown.child_token());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        Self {
            base_url,
            github,
            http: reqwest::Client::builder()
                .redirect(Policy::none())
                .build()
                .unwrap(),
            _shutdown: shutdown,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// A user with a personal installation of one private repository.
    pub async fn given_user(&self, user: &FakeUser) {
        self.given_oauth(user).await;
        self.given_installations(user, &[installation_json(user)])
            .await;
        self.given_installation_repositories(user, 1, true).await;
    }

    /// GitHub's sign-in and identity endpoints for a user.
    pub async fn given_oauth(&self, user: &FakeUser) {
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .and(body_string_contains(format!("code={}", user.code)))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": user.user_token(),
                "refresh_token": user.refresh_token(),
            })))
            .mount(&self.github)
            .await;
        let (name, value) = user.auth(user.user_token());
        Mock::given(method("GET"))
            .and(path("/user"))
            .and(header(name, value.as_str()))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"id": user.id, "login": user.login})),
            )
            .mount(&self.github)
            .await;
    }

    pub async fn given_installations(&self, user: &FakeUser, installations: &[Value]) {
        let (name, value) = user.auth(user.user_token());
        Mock::given(method("GET"))
            .and(path("/user/installations"))
            .and(header(name, value.as_str()))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"installations": installations})),
            )
            .mount(&self.github)
            .await;
    }

    pub async fn given_installation_repositories(
        &self,
        user: &FakeUser,
        count: u64,
        private: bool,
    ) {
        Mock::given(method("POST"))
            .and(path(format!(
                "/app/installations/{}/access_tokens",
                user.installation_id
            )))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(json!({"token": user.installation_token()})),
            )
            .mount(&self.github)
            .await;
        let (name, value) = user.auth(user.installation_token());
        Mock::given(method("GET"))
            .and(path("/installation/repositories"))
            .and(header(name, value.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "total_count": count,
                "repositories": [{
                    "name": "todos",
                    "full_name": format!("{}/todos", user.login),
                    "private": private,
                    "html_url": format!("https://github.test/{}/todos", user.login),
                    "default_branch": "main",
                    "owner": {"login": user.login},
                }],
            })))
            .mount(&self.github)
            .await;
    }

    /// Issues in a user's repository, reachable only with that user's
    /// installation token. Any other combination falls through to a 404.
    pub async fn given_issues(&self, user: &FakeUser, issues: Value) {
        let (name, value) = user.auth(user.installation_token());
        Mock::given(method("GET"))
            .and(path(format!("/repos/{}/todos/issues", user.login)))
            .and(header(name, value.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(issues))
            .mount(&self.github)
            .await;
    }

    pub async fn given_issue(&self, user: &FakeUser, number: u64, issue: Value) {
        let (name, value) = user.auth(user.installation_token());
        Mock::given(method("GET"))
            .and(path(format!("/repos/{}/todos/issues/{number}", user.login)))
            .and(header(name, value.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue))
            .mount(&self.github)
            .await;
        let (name, value) = user.auth(user.installation_token());
        Mock::given(method("GET"))
            .and(path_regex(format!(
                r"^/repos/{}/todos/issues/{number}/comments$",
                user.login
            )))
            .and(header(name, value.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&self.github)
            .await;
    }

    pub async fn register(&self) -> String {
        let response: Value = self
            .http
            .post(self.url("/register"))
            .json(&json!({"client_name": "Test client", "redirect_uris": [CLIENT_REDIRECT]}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        response["client_id"].as_str().unwrap().to_string()
    }

    pub fn authorize_url(&self, client_id: &str) -> String {
        let mut url = Url::parse(&self.url("/authorize")).unwrap();
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", CLIENT_REDIRECT)
            .append_pair("code_challenge", &challenge(VERIFIER))
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", "client-state")
            .append_pair("resource", &self.url("/mcp"));
        url.into()
    }

    /// Runs the browser side up to GitHub's sign-in.
    pub async fn browser_to_github(&self, client_id: &str) -> BrowserAtGitHub {
        let page = self
            .http
            .get(self.authorize_url(client_id))
            .send()
            .await
            .unwrap();
        assert_eq!(page.status(), 200);
        let flow_cookie = flow_cookie(&page);
        let html = page.text().await.unwrap();

        let consent = self
            .http
            .post(self.url("/authorize/consent"))
            .header("cookie", &flow_cookie)
            .form(&[
                ("consent", hidden_consent(&html).as_str()),
                ("decision", "allow"),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(consent.status(), 303, "consent should redirect to GitHub");
        let state = query_param(&location(&consent), "state").unwrap();
        BrowserAtGitHub { flow_cookie, state }
    }

    /// GitHub redirecting the signed-in user back.
    pub async fn github_redirect(
        &self,
        browser: &BrowserAtGitHub,
        code: &str,
    ) -> reqwest::Response {
        let mut url = Url::parse(&self.url("/oauth/github/callback")).unwrap();
        url.query_pairs_mut()
            .append_pair("code", code)
            .append_pair("state", &browser.state);
        self.http
            .get(url)
            .header("cookie", &browser.flow_cookie)
            .send()
            .await
            .unwrap()
    }

    pub async fn exchange_code(
        &self,
        client_id: &str,
        code: &str,
        verifier: &str,
    ) -> reqwest::Response {
        self.http
            .post(self.url("/token"))
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", client_id),
                ("code", code),
                ("redirect_uri", CLIENT_REDIRECT),
                ("code_verifier", verifier),
            ])
            .send()
            .await
            .unwrap()
    }

    /// Full happy path for a user who already has an installation.
    pub async fn sign_in(&self, user: &FakeUser) -> Tokens {
        let client_id = self.register().await;
        let browser = self.browser_to_github(&client_id).await;
        let done = self.github_redirect(&browser, user.code).await;
        assert_eq!(done.status(), 303, "callback should return to the client");
        let code = query_param(&location(&done), "code").expect("redirect carries a code");
        let tokens: Value = self
            .exchange_code(&client_id, &code, VERIFIER)
            .await
            .json()
            .await
            .unwrap();
        Tokens {
            client_id,
            access: tokens["access_token"].as_str().unwrap().to_string(),
            refresh: tokens["refresh_token"].as_str().unwrap().to_string(),
        }
    }

    /// Calls a tool over the real MCP endpoint.
    pub async fn call_tool(&self, access_token: &str, tool: &str, arguments: Value) -> ToolOutcome {
        let response = self
            .http
            .post(self.url("/mcp"))
            .bearer_auth(access_token)
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2025-06-18")
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": tool, "arguments": arguments},
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: Value = response.json().await.unwrap();
        let result = &body["result"];
        assert!(result.is_object(), "expected a tool result, got {body}");
        ToolOutcome {
            is_error: result["isError"].as_bool().unwrap_or(false),
            text: result["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        }
    }
}

pub struct BrowserAtGitHub {
    pub flow_cookie: String,
    pub state: String,
}

pub struct Tokens {
    pub client_id: String,
    pub access: String,
    pub refresh: String,
}

#[derive(Debug)]
pub struct ToolOutcome {
    pub is_error: bool,
    pub text: String,
}

pub fn installation_json(user: &FakeUser) -> Value {
    json!({
        "id": user.installation_id,
        "target_type": "User",
        "suspended_at": null,
        "account": {"id": user.id},
    })
}

pub fn issue_json(number: u64, title: &str) -> Value {
    json!({
        "number": number,
        "title": title,
        "body": "",
        "state": "open",
        "state_reason": null,
        "labels": [],
        "created_at": "2026-10-30T10:00:00Z",
        "updated_at": "2026-10-30T10:00:00Z",
        "html_url": format!("https://github.test/issues/{number}"),
    })
}

pub fn challenge(verifier: &str) -> String {
    B64.encode(Sha256::digest(verifier.as_bytes()))
}

pub fn location(response: &reqwest::Response) -> String {
    response.headers()["location"].to_str().unwrap().to_string()
}

pub fn query_param(url: &str, name: &str) -> Option<String> {
    Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

/// The cookie named `prefix`, in `name=value` form, from a response.
pub fn response_cookie(response: &reqwest::Response, prefix: &str) -> Option<String> {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with(prefix))
        .map(|value| value.split(';').next().unwrap().to_string())
}

fn flow_cookie(response: &reqwest::Response) -> String {
    response_cookie(response, "__Host-todo_flow=").expect("flow cookie is set")
}

pub fn hidden_consent(html: &str) -> String {
    let marker = "name=\"consent\" value=\"";
    let start = html.find(marker).expect("consent token in page") + marker.len();
    let end = html[start..].find('"').unwrap();
    html[start..start + end].to_string()
}
