//! Multi-tenancy over the real MCP endpoint. The fake GitHub answers a
//! repository only for that owner's installation token, so any cross-tenant
//! request shows up as a failure rather than as someone else's data.

mod support;

use serde_json::{Value, json};
use support::*;
use wiremock::matchers::{body_json_string, body_partial_json, method, path};
use wiremock::{Mock, ResponseTemplate};

async fn alice_and_bob(h: &Harness) -> (Tokens, Tokens) {
    h.given_user(&ALICE).await;
    h.given_user(&BOB).await;
    let alice = h.sign_in(&ALICE).await;
    let bob = h.sign_in(&BOB).await;
    (alice, bob)
}

fn titles(outcome: &ToolOutcome) -> Vec<String> {
    let page: Value = serde_json::from_str(&outcome.text).unwrap();
    page["todos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|todo| todo["title"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn each_token_sees_only_its_own_todos() {
    let h = Harness::start().await;
    let (alice, bob) = alice_and_bob(&h).await;
    h.given_issues(&ALICE, json!([issue_json(1, "Alice's secret")]))
        .await;
    h.given_issues(&BOB, json!([issue_json(1, "Bob's secret")]))
        .await;

    let seen_by_alice = h.call_tool(&alice.access, "todo_list", json!({})).await;
    let seen_by_bob = h.call_tool(&bob.access, "todo_list", json!({})).await;

    assert_eq!(titles(&seen_by_alice), ["Alice's secret"]);
    assert_eq!(titles(&seen_by_bob), ["Bob's secret"]);
}

#[tokio::test]
async fn the_same_todo_id_resolves_per_tenant() {
    let h = Harness::start().await;
    let (alice, bob) = alice_and_bob(&h).await;
    h.given_issue(&ALICE, 1, issue_json(1, "Alice's secret"))
        .await;

    let own = h
        .call_tool(&alice.access, "todo_get", json!({"id": 1}))
        .await;
    let other = h.call_tool(&bob.access, "todo_get", json!({"id": 1})).await;

    assert!(own.text.contains("Alice's secret"));
    assert!(other.is_error);
    assert!(!other.text.contains("secret"));
}

#[tokio::test]
async fn tool_arguments_cannot_redirect_a_request_to_another_repository() {
    let h = Harness::start().await;
    let (alice, _bob) = alice_and_bob(&h).await;
    h.given_issues(&ALICE, json!([issue_json(1, "Alice's own")]))
        .await;
    h.given_issues(&BOB, json!([issue_json(1, "Bob's secret")]))
        .await;

    let outcome = h
        .call_tool(
            &alice.access,
            "todo_list",
            json!({"owner": "bob", "repo": "todos", "repository": "bob/todos", "installation_id": 5002}),
        )
        .await;

    assert_eq!(titles(&outcome), ["Alice's own"]);
}

#[tokio::test]
async fn a_sealed_token_from_another_deployment_is_refused() {
    let h = Harness::start().await;
    let other = Harness::start().await;
    other.given_user(&ALICE).await;
    let token = other.sign_in(&ALICE).await;

    // Same key material, but the token's audience is the other deployment.
    let response = h
        .http
        .post(h.url("/mcp"))
        .bearer_auth(&token.access)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn tools_are_discoverable_and_never_ask_for_a_repository() {
    let h = Harness::start().await;
    h.given_user(&ALICE).await;
    let tokens = h.sign_in(&ALICE).await;

    let response: Value = h
        .http
        .post(h.url("/mcp"))
        .bearer_auth(&tokens.access)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let tools = response["result"]["tools"].as_array().expect("tool list");
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for expected in [
        "todo_list",
        "todo_get",
        "todo_create",
        "todo_update",
        "todo_complete",
        "todo_drop",
        "todo_reopen",
        "todo_comment",
        "labels_list",
        "label_delete",
        "todo_repository",
        "file_list",
        "file_read",
        "file_edit",
        "file_write",
        "file_delete",
    ] {
        assert!(names.contains(&expected), "missing {expected}");
    }
    for tool in tools {
        let schema = tool["inputSchema"].to_string();
        for forbidden in [
            "\"owner\"",
            "\"repo\"",
            "\"repository\"",
            "\"installation_id\"",
        ] {
            assert!(
                !schema.contains(forbidden),
                "{} takes {forbidden}",
                tool["name"]
            );
        }
    }
}

#[tokio::test]
async fn pull_requests_are_not_todos() {
    let h = Harness::start().await;
    h.given_user(&ALICE).await;
    let tokens = h.sign_in(&ALICE).await;
    let mut pull_request = issue_json(2, "A pull request");
    pull_request["pull_request"] = json!({"url": "https://github.test/pulls/2"});
    h.given_issues(
        &ALICE,
        json!([issue_json(1, "A todo"), pull_request.clone()]),
    )
    .await;
    h.given_issue(&ALICE, 2, pull_request).await;

    let listed = h.call_tool(&tokens.access, "todo_list", json!({})).await;
    let fetched = h
        .call_tool(&tokens.access, "todo_get", json!({"id": 2}))
        .await;

    assert_eq!(titles(&listed), ["A todo"]);
    assert!(fetched.is_error);
}

#[tokio::test]
async fn creating_a_todo_creates_its_labels_and_writes_the_due_date() {
    let h = Harness::start().await;
    h.given_user(&ALICE).await;
    let tokens = h.sign_in(&ALICE).await;
    Mock::given(method("GET"))
        .and(path("/repos/alice/todos/labels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"name": "priority:high"}])))
        .mount(&h.github)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/alice/todos/labels"))
        .and(body_partial_json(json!({"name": "list:home"})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"name": "list:home"})))
        .expect(1)
        .mount(&h.github)
        .await;
    let mut created = issue_json(9, "Water plants");
    created["body"] = json!("Due: 2026-11-03");
    created["labels"] = json!([{"name": "priority:high"}, {"name": "list:home"}]);
    Mock::given(method("POST"))
        .and(path("/repos/alice/todos/issues"))
        .and(body_json_string(
            r#"{"title":"Water plants","body":"Due: 2026-11-03","labels":["priority:high","list:home"]}"#,
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(created))
        .expect(1)
        .mount(&h.github)
        .await;

    let outcome = h
        .call_tool(
            &tokens.access,
            "todo_create",
            json!({"title": "Water plants", "priority": "high", "list": "home", "due": "2026-11-03"}),
        )
        .await;

    assert!(!outcome.is_error, "{}", outcome.text);
    let todo: Value = serde_json::from_str(&outcome.text).unwrap();
    assert_eq!(todo["id"], 9);
    assert_eq!(todo["due"], "2026-11-03");
    assert_eq!(todo["priority"], "high");
    assert_eq!(todo["list"], "home");
}

#[tokio::test]
async fn completing_closes_the_issue_as_completed() {
    let h = Harness::start().await;
    h.given_user(&ALICE).await;
    let tokens = h.sign_in(&ALICE).await;
    h.given_issue(&ALICE, 1, issue_json(1, "Water plants"))
        .await;
    let mut closed = issue_json(1, "Water plants");
    closed["state"] = json!("closed");
    closed["state_reason"] = json!("completed");
    Mock::given(method("PATCH"))
        .and(path("/repos/alice/todos/issues/1"))
        .and(body_partial_json(
            json!({"state": "closed", "state_reason": "completed"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(closed))
        .expect(1)
        .mount(&h.github)
        .await;

    let outcome = h
        .call_tool(&tokens.access, "todo_complete", json!({"id": 1}))
        .await;

    let todo: Value = serde_json::from_str(&outcome.text).unwrap();
    assert_eq!(todo["state"], "done");
}

#[tokio::test]
async fn invalid_arguments_come_back_as_a_tool_error_without_calling_github() {
    let h = Harness::start().await;
    h.given_user(&ALICE).await;
    let tokens = h.sign_in(&ALICE).await;

    let outcome = h
        .call_tool(
            &tokens.access,
            "todo_create",
            json!({"title": "x", "due": "next tuesday"}),
        )
        .await;

    assert!(outcome.is_error);
    assert!(outcome.text.contains("YYYY-MM-DD"));
}

#[tokio::test]
async fn a_client_can_initialize_over_the_stateless_transport() {
    let h = Harness::start().await;
    h.given_user(&ALICE).await;
    let tokens = h.sign_in(&ALICE).await;

    let response = h
        .http
        .post(h.url("/mcp"))
        .bearer_auth(&tokens.access)
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1"},
            },
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert!(!response.headers().contains_key("mcp-session-id"));
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["result"]["serverInfo"]["name"], "personal-todo-mcp");
    assert!(body["result"]["capabilities"]["tools"].is_object());
}

#[tokio::test]
async fn browser_clients_can_preflight_and_read_the_challenge() {
    let h = Harness::start().await;

    let preflight = h
        .http
        .request(reqwest::Method::OPTIONS, h.url("/mcp"))
        .header("origin", "https://app.example")
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "authorization")
        .send()
        .await
        .unwrap();
    assert_eq!(preflight.status(), 200);
    assert_eq!(preflight.headers()["access-control-allow-origin"], "*");

    let rejected = h
        .http
        .post(h.url("/mcp"))
        .header("origin", "https://app.example")
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), 401);
    let exposed = rejected.headers()["access-control-expose-headers"]
        .to_str()
        .unwrap();
    assert!(exposed.to_lowercase().contains("www-authenticate"));
}

async fn tools_list_with_host(h: &Harness, token: &str, host: &str) -> reqwest::StatusCode {
    h.http
        .post(h.url("/mcp"))
        .bearer_auth(token)
        .header("host", host)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
        .send()
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn only_the_configured_public_host_reaches_the_tools() {
    let h = Harness::start().await;
    h.given_user(&ALICE).await;
    let tokens = h.sign_in(&ALICE).await;

    assert_eq!(
        tools_list_with_host(&h, &tokens.access, PUBLIC_HOST).await,
        200
    );
    assert_eq!(
        tools_list_with_host(&h, &tokens.access, "evil.example").await,
        403
    );
}
