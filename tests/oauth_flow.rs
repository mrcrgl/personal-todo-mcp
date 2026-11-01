//! The onboarding flow, and the ways it must refuse to work.

mod support;

use serde_json::{Value, json};
use support::*;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test]
async fn unauthenticated_mcp_requests_point_at_the_metadata() {
    let h = Harness::start().await;
    let response = h.http.post(h.url("/mcp")).send().await.unwrap();

    assert_eq!(response.status(), 401);
    let challenge = response.headers()["www-authenticate"].to_str().unwrap();
    assert!(challenge.starts_with("Bearer resource_metadata=\""));
    assert!(challenge.contains("/.well-known/oauth-protected-resource/mcp"));
}

#[tokio::test]
async fn discovery_leads_from_the_resource_to_the_authorization_server() {
    let h = Harness::start().await;

    let resource: Value = h
        .http
        .get(h.url("/.well-known/oauth-protected-resource/mcp"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resource["resource"], h.url("/mcp"));
    assert_eq!(resource["authorization_servers"][0], h.base_url);

    let server: Value = h
        .http
        .get(h.url("/.well-known/oauth-authorization-server"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(server["issuer"], h.base_url);
    assert_eq!(server["registration_endpoint"], h.url("/register"));
    assert_eq!(server["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(
        server["token_endpoint_auth_methods_supported"],
        json!(["none"])
    );
}

#[tokio::test]
async fn registration_rejects_unsafe_redirects() {
    let h = Harness::start().await;
    for uri in ["http://evil.example/cb", "javascript:alert(1)"] {
        let response = h
            .http
            .post(h.url("/register"))
            .json(&json!({"redirect_uris": [uri]}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"], "invalid_redirect_uri");
    }
}

#[tokio::test]
async fn a_returning_user_connects_without_reinstalling() {
    let h = Harness::start().await;
    h.given_user(&ALICE).await;

    let tokens = h.sign_in(&ALICE).await;

    assert!(!tokens.access.is_empty());
    assert!(!tokens.refresh.is_empty());
}

#[tokio::test]
async fn the_client_gets_its_state_and_our_issuer_back() {
    let h = Harness::start().await;
    h.given_user(&ALICE).await;
    let client_id = h.register().await;
    let browser = h.browser_to_github(&client_id).await;

    let done = h.github_redirect(&browser, ALICE.code).await;

    let target = location(&done);
    assert!(target.starts_with(CLIENT_REDIRECT));
    assert_eq!(query_param(&target, "state").unwrap(), "client-state");
    assert_eq!(query_param(&target, "iss").unwrap(), h.base_url);
}

#[tokio::test]
async fn a_new_user_is_sent_to_install_and_returns_to_the_client() {
    let h = Harness::start().await;
    h.given_oauth(&ALICE).await;
    let client_id = h.register().await;
    let browser = h.browser_to_github(&client_id).await;

    // First listing: nothing installed yet. Mounted to expire after one use.
    Mock::given(method("GET"))
        .and(path("/user/installations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"installations": []})))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&h.github)
        .await;
    let to_install = h.github_redirect(&browser, ALICE.code).await;

    assert_eq!(to_install.status(), 303);
    assert!(location(&to_install).ends_with("/apps/personal-todos/installations/new"));
    let install_cookie = response_cookie(&to_install, "__Host-todo_install=")
        .expect("flow state survives the installation in a cookie");

    // The user installs; GitHub sends them to the setup URL.
    h.given_installations(&ALICE, &[installation_json(&ALICE)])
        .await;
    h.given_installation_repositories(&ALICE, 1, true).await;
    let back = h
        .http
        .get(h.url("/oauth/github/setup?installation_id=5001&setup_action=install"))
        .header("cookie", &install_cookie)
        .send()
        .await
        .unwrap();

    assert_eq!(back.status(), 303);
    let code = query_param(&location(&back), "code").expect("returned to the client with a code");
    let tokens = h.exchange_code(&client_id, &code, VERIFIER).await;
    assert_eq!(tokens.status(), 200);
    assert!(
        response_cookie(&back, "__Host-todo_install=").is_some_and(|c| c.ends_with('=')),
        "the install cookie holds GitHub tokens and must be cleared"
    );
}

#[tokio::test]
async fn a_spoofed_installation_id_is_ignored() {
    let h = Harness::start().await;
    h.given_oauth(&ALICE).await;
    h.given_installations(&ALICE, &[]).await;
    let client_id = h.register().await;
    let browser = h.browser_to_github(&client_id).await;
    let to_install = h.github_redirect(&browser, ALICE.code).await;
    let install_cookie = response_cookie(&to_install, "__Host-todo_install=").unwrap();

    // Alice claims Bob's installation. GitHub says she has none of her own.
    let response = h
        .http
        .get(h.url(&format!(
            "/oauth/github/setup?installation_id={}&setup_action=install",
            BOB.installation_id
        )))
        .header("cookie", &install_cookie)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert!(!response.headers().contains_key("location"));
    assert!(
        response
            .text()
            .await
            .unwrap()
            .contains("No installation was found")
    );
}

#[tokio::test]
async fn an_organization_installation_is_not_accepted() {
    let h = Harness::start().await;
    h.given_oauth(&ALICE).await;
    h.given_installations(
        &ALICE,
        &[json!({
            "id": 9000,
            "target_type": "Organization",
            "suspended_at": null,
            "account": {"id": 777},
        })],
    )
    .await;
    let client_id = h.register().await;
    let browser = h.browser_to_github(&client_id).await;

    let response = h.github_redirect(&browser, ALICE.code).await;

    assert!(location(&response).contains("/installations/new"));
}

#[tokio::test]
async fn a_public_repository_blocks_the_flow_with_a_way_out() {
    let h = Harness::start().await;
    h.given_oauth(&ALICE).await;
    h.given_installations(&ALICE, &[installation_json(&ALICE)])
        .await;
    h.given_installation_repositories(&ALICE, 1, false).await;
    let client_id = h.register().await;
    let browser = h.browser_to_github(&client_id).await;

    let response = h.github_redirect(&browser, ALICE.code).await;

    assert_eq!(response.status(), 200);
    let html = response.text().await.unwrap();
    assert!(html.contains("is public"));
    assert!(html.contains("/settings/installations/5001"));
    assert!(html.contains("/oauth/github/setup"));
}

#[tokio::test]
async fn several_repositories_block_the_flow() {
    let h = Harness::start().await;
    h.given_oauth(&ALICE).await;
    h.given_installations(&ALICE, &[installation_json(&ALICE)])
        .await;
    h.given_installation_repositories(&ALICE, 3, true).await;
    let client_id = h.register().await;
    let browser = h.browser_to_github(&client_id).await;

    let response = h.github_redirect(&browser, ALICE.code).await;

    assert!(response.text().await.unwrap().contains("exactly one"));
}

#[tokio::test]
async fn consent_is_shown_before_anything_reaches_github() {
    let h = Harness::start().await;
    let client_id = h.register().await;

    let page = h
        .http
        .get(h.authorize_url(&client_id))
        .send()
        .await
        .unwrap();

    assert_eq!(page.status(), 200);
    assert_eq!(page.headers()["x-frame-options"], "DENY");
    let html = page.text().await.unwrap();
    assert!(html.contains("client.example"));
    assert!(html.contains("Test client"));
}

#[tokio::test]
async fn a_consent_link_from_another_browser_does_not_work() {
    let h = Harness::start().await;
    let client_id = h.register().await;
    let page = h
        .http
        .get(h.authorize_url(&client_id))
        .send()
        .await
        .unwrap();
    let html = page.text().await.unwrap();

    // The attacker's consent token, submitted from the victim's browser, which
    // holds a different nonce.
    let response = h
        .http
        .post(h.url("/authorize/consent"))
        .header("cookie", "__Host-todo_flow=victims-nonce")
        .form(&[
            ("consent", hidden_consent(&html).as_str()),
            ("decision", "allow"),
        ])
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 400);
    assert!(!response.headers().contains_key("location"));
}

#[tokio::test]
async fn a_github_callback_from_another_browser_does_not_work() {
    let h = Harness::start().await;
    h.given_user(&ALICE).await;
    let client_id = h.register().await;
    let mut browser = h.browser_to_github(&client_id).await;
    browser.flow_cookie = "__Host-todo_flow=someone-else".into();

    let response = h.github_redirect(&browser, ALICE.code).await;

    assert_eq!(response.status(), 400);
}

#[tokio::test]
async fn declining_consent_tells_the_client() {
    let h = Harness::start().await;
    let client_id = h.register().await;
    let page = h
        .http
        .get(h.authorize_url(&client_id))
        .send()
        .await
        .unwrap();
    let cookie = response_cookie(&page, "__Host-todo_flow=").unwrap();
    let html = page.text().await.unwrap();

    let response = h
        .http
        .post(h.url("/authorize/consent"))
        .header("cookie", &cookie)
        .form(&[
            ("consent", hidden_consent(&html).as_str()),
            ("decision", "deny"),
        ])
        .send()
        .await
        .unwrap();

    let target = location(&response);
    assert!(target.starts_with(CLIENT_REDIRECT));
    assert_eq!(query_param(&target, "error").unwrap(), "access_denied");
}

#[tokio::test]
async fn an_unregistered_redirect_never_receives_a_redirect() {
    let h = Harness::start().await;
    let client_id = h.register().await;
    let url = h
        .authorize_url(&client_id)
        .replace("client.example", "evil.example");

    let response = h.http.get(url).send().await.unwrap();

    assert_eq!(response.status(), 400);
    assert!(!response.headers().contains_key("location"));
}

#[tokio::test]
async fn a_made_up_client_id_is_refused() {
    let h = Harness::start().await;
    let response = h.http.get(h.authorize_url("forged")).send().await.unwrap();
    assert_eq!(response.status(), 400);
}

#[tokio::test]
async fn pkce_is_mandatory() {
    let h = Harness::start().await;
    let client_id = h.register().await;
    let url = h
        .authorize_url(&client_id)
        .replace("code_challenge_method=S256", "code_challenge_method=plain");

    let response = h.http.get(url).send().await.unwrap();

    assert_eq!(response.status(), 303);
    assert_eq!(
        query_param(&location(&response), "error").unwrap(),
        "invalid_request"
    );
}

#[tokio::test]
async fn a_token_for_another_resource_is_refused() {
    let h = Harness::start().await;
    let client_id = h.register().await;
    let url = format!(
        "{}&resource=https%3A%2F%2Fother.example%2Fmcp",
        h.authorize_url(&client_id)
    )
    .replacen(
        &format!(
            "resource={}",
            url::form_urlencoded::byte_serialize(h.url("/mcp").as_bytes()).collect::<String>()
        ),
        "x=1",
        1,
    );

    let response = h.http.get(url).send().await.unwrap();

    assert_eq!(
        query_param(&location(&response), "error").unwrap(),
        "invalid_target"
    );
}

#[tokio::test]
async fn a_stolen_code_is_useless_without_the_verifier() {
    let h = Harness::start().await;
    h.given_user(&ALICE).await;
    let client_id = h.register().await;
    let browser = h.browser_to_github(&client_id).await;
    let done = h.github_redirect(&browser, ALICE.code).await;
    let code = query_param(&location(&done), "code").unwrap();

    let response = h.exchange_code(&client_id, &code, &"a".repeat(43)).await;

    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"], "invalid_grant");
}

#[tokio::test]
async fn a_code_only_works_for_the_client_it_was_issued_to() {
    let h = Harness::start().await;
    h.given_user(&ALICE).await;
    let client_id = h.register().await;
    let other_client = h.register().await;
    let browser = h.browser_to_github(&client_id).await;
    let done = h.github_redirect(&browser, ALICE.code).await;
    let code = query_param(&location(&done), "code").unwrap();

    let response = h.exchange_code(&other_client, &code, VERIFIER).await;

    assert_eq!(response.status(), 400);
}

#[tokio::test]
async fn an_access_token_is_not_a_code() {
    let h = Harness::start().await;
    h.given_user(&ALICE).await;
    let tokens = h.sign_in(&ALICE).await;

    let response = h
        .exchange_code(&tokens.client_id, &tokens.access, VERIFIER)
        .await;

    assert_eq!(response.status(), 400);
}

#[tokio::test]
async fn a_refresh_token_is_not_an_access_token() {
    let h = Harness::start().await;
    h.given_user(&ALICE).await;
    let tokens = h.sign_in(&ALICE).await;

    let response = h
        .http
        .post(h.url("/mcp"))
        .bearer_auth(&tokens.refresh)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 401);
    let challenge = response.headers()["www-authenticate"].to_str().unwrap();
    assert!(challenge.contains("invalid_token"));
}

async fn refresh(h: &Harness, tokens: &Tokens) -> reqwest::Response {
    h.http
        .post(h.url("/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", tokens.client_id.as_str()),
            ("refresh_token", tokens.refresh.as_str()),
        ])
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn refresh_rotates_through_github() {
    let h = Harness::start().await;
    h.given_user(&ALICE).await;
    let tokens = h.sign_in(&ALICE).await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=ghr_alice"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": ALICE.user_token(),
            "refresh_token": "ghr_alice_rotated",
        })))
        .mount(&h.github)
        .await;

    let response = refresh(&h, &tokens).await;

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_ne!(body["refresh_token"], tokens.refresh.as_str());
    assert_eq!(body["expires_in"], 3600);
}

#[tokio::test]
async fn a_replayed_refresh_token_is_rejected() {
    let h = Harness::start().await;
    h.given_user(&ALICE).await;
    let tokens = h.sign_in(&ALICE).await;
    // GitHub reports a spent token in the body of a 200.
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"error": "bad_refresh_token"})),
        )
        .mount(&h.github)
        .await;

    let response = refresh(&h, &tokens).await;

    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"], "invalid_grant");
}

#[tokio::test]
async fn refresh_stops_working_after_uninstalling() {
    let h = Harness::start().await;
    h.given_oauth(&ALICE).await;
    h.given_installation_repositories(&ALICE, 1, true).await;
    Mock::given(method("GET"))
        .and(path("/user/installations"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"installations": [installation_json(&ALICE)]})),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&h.github)
        .await;
    let tokens = h.sign_in(&ALICE).await;

    h.given_installations(&ALICE, &[]).await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": ALICE.user_token(),
            "refresh_token": "ghr_alice_rotated",
        })))
        .mount(&h.github)
        .await;

    let response = refresh(&h, &tokens).await;

    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.unwrap();
    assert!(
        body["error_description"]
            .as_str()
            .unwrap()
            .contains("no longer installed")
    );
}

#[tokio::test]
async fn a_github_outage_during_refresh_is_retryable() {
    let h = Harness::start().await;
    h.given_user(&ALICE).await;
    let tokens = h.sign_in(&ALICE).await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&h.github)
        .await;

    let response = refresh(&h, &tokens).await;

    assert_eq!(response.status(), 503);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"], "temporarily_unavailable");
}

#[tokio::test]
async fn token_responses_are_not_cacheable() {
    let h = Harness::start().await;
    h.given_user(&ALICE).await;
    let client_id = h.register().await;
    let browser = h.browser_to_github(&client_id).await;
    let done = h.github_redirect(&browser, ALICE.code).await;
    let code = query_param(&location(&done), "code").unwrap();

    let response = h.exchange_code(&client_id, &code, VERIFIER).await;

    assert_eq!(response.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn two_flows_can_run_side_by_side_in_one_browser() {
    let h = Harness::start().await;
    h.given_user(&ALICE).await;
    let client_id = h.register().await;
    let first = h.browser_to_github(&client_id).await;
    let page = h
        .http
        .get(h.authorize_url(&client_id))
        .header("cookie", &first.flow_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response_cookie(&page, "__Host-todo_flow=").unwrap(),
        first.flow_cookie,
        "the second flow reuses the browser's nonce"
    );

    let done = h.github_redirect(&first, ALICE.code).await;

    assert_eq!(done.status(), 303);
    assert!(
        response_cookie(&done, "__Host-todo_flow=").is_none(),
        "completing one flow must not end the other"
    );
}
