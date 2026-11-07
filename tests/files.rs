//! File tools over the real MCP endpoint. As in `tenancy.rs`, the fake GitHub
//! answers a repository only for its owner's installation token.

mod support;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value, json};
use support::*;
use wiremock::matchers::{
    body_partial_json, body_string_contains, header, method, path, query_param,
};
use wiremock::{Mock, ResponseTemplate};

fn auth(user: &FakeUser) -> String {
    format!("Bearer {}", user.installation_token())
}

fn content_json(text: &str, sha: &str) -> Value {
    json!({
        "type": "file",
        "encoding": "base64",
        "size": text.len(),
        "sha": sha,
        "content": B64.encode(text),
    })
}

/// A file in a user's repository, readable only with that user's token.
async fn given_file(h: &Harness, user: &FakeUser, file: &str, text: &str, sha: &str) {
    Mock::given(method("GET"))
        .and(path(format!(
            "/repos/{}/todos/contents/files/{file}",
            user.login
        )))
        .and(header("authorization", auth(user).as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(content_json(text, sha)))
        .mount(&h.github)
        .await;
}

fn written(sha: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"content": {"sha": sha}, "commit": {}}))
}

/// Requests that reached a repository, as `METHOD path`.
async fn repository_requests(h: &Harness) -> Vec<String> {
    h.github
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.url.path().starts_with("/repos/"))
        .map(|request| format!("{} {}", request.method, request.url.path()))
        .collect()
}

async fn signed_in(h: &Harness, user: &FakeUser) -> Tokens {
    h.given_user(user).await;
    h.sign_in(user).await
}

#[tokio::test]
async fn file_read_shows_numbered_lines_and_the_sha() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    given_file(
        &h,
        &ALICE,
        "projects/house.md",
        "# House\n- roof\n- windows\n",
        "v1",
    )
    .await;

    let outcome = h
        .call_tool(
            &alice.access,
            "file_read",
            json!({"path": "projects/house.md", "offset": 2, "limit": 1}),
        )
        .await;

    assert!(!outcome.is_error, "{}", outcome.text);
    assert_eq!(
        outcome.text,
        "path: projects/house.md\nsha: v1\nlines 2-2 of 3. More follows: read on with offset 3.\n\n     2\t- roof\n"
    );
}

#[tokio::test]
async fn file_list_reads_the_default_branch_tree() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    Mock::given(method("GET"))
        .and(path("/repos/alice/todos/git/trees/main"))
        .and(query_param("recursive", "1"))
        .and(header("authorization", auth(&ALICE).as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "truncated": false,
            "tree": [
                {"path": "README.md", "type": "blob", "size": 10},
                {"path": "files/b.md", "type": "blob", "size": 2},
                {"path": "files/a.md", "type": "blob", "size": 1},
                {"path": "files/img.png", "type": "blob", "size": 9},
            ],
        })))
        .mount(&h.github)
        .await;

    let outcome = h.call_tool(&alice.access, "file_list", json!({})).await;

    assert!(!outcome.is_error, "{}", outcome.text);
    let listing: Value = serde_json::from_str(&outcome.text).unwrap();
    assert_eq!(
        listing,
        json!({"files": [{"path": "a.md", "size": 1}, {"path": "b.md", "size": 2}], "truncated": false})
    );
}

#[tokio::test]
async fn an_empty_repository_lists_no_files() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    Mock::given(method("GET"))
        .and(path("/repos/alice/todos/git/trees/main"))
        .respond_with(
            ResponseTemplate::new(409)
                .set_body_json(json!({"message": "Git Repository is empty."})),
        )
        .mount(&h.github)
        .await;

    let outcome = h.call_tool(&alice.access, "file_list", json!({})).await;

    assert!(!outcome.is_error, "{}", outcome.text);
    assert!(outcome.text.contains(r#""files":[]"#));
}

#[tokio::test]
async fn file_edit_writes_the_edited_file_against_the_sha_it_read() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    given_file(&h, &ALICE, "notes.md", "# Notes\n- [ ] call\n", "v1").await;
    Mock::given(method("PUT"))
        .and(path("/repos/alice/todos/contents/files/notes.md"))
        .and(header("authorization", auth(&ALICE).as_str()))
        .and(body_partial_json(json!({
            "message": "Update files/notes.md",
            "sha": "v1",
            "content": B64.encode("# Notes\n- [x] call\n"),
        })))
        .respond_with(written("v2"))
        .expect(1)
        .mount(&h.github)
        .await;

    let outcome = h
        .call_tool(
            &alice.access,
            "file_edit",
            json!({"path": "notes.md", "edits": [{"old_string": "- [ ] call", "new_string": "- [x] call"}]}),
        )
        .await;

    assert!(!outcome.is_error, "{}", outcome.text);
    assert!(outcome.text.contains("sha: v2"));
    assert!(outcome.text.contains("     2\t- [x] call"));
}

#[tokio::test]
async fn file_edit_reports_a_conflict_after_one_retry() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    given_file(&h, &ALICE, "notes.md", "a\n", "v1").await;
    Mock::given(method("PUT"))
        .and(path("/repos/alice/todos/contents/files/notes.md"))
        .respond_with(
            ResponseTemplate::new(409)
                .set_body_json(json!({"message": "files/notes.md does not match v1"})),
        )
        .expect(2)
        .mount(&h.github)
        .await;

    let outcome = h
        .call_tool(
            &alice.access,
            "file_edit",
            json!({"path": "notes.md", "edits": [{"old_string": "a", "new_string": "b"}]}),
        )
        .await;

    assert!(outcome.is_error);
    assert!(
        outcome.text.contains("changed since it was read"),
        "{}",
        outcome.text
    );
}

#[tokio::test]
async fn file_edit_explains_a_missing_match_without_writing() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    given_file(&h, &ALICE, "notes.md", "a\n", "v1").await;

    let outcome = h
        .call_tool(
            &alice.access,
            "file_edit",
            json!({"path": "notes.md", "edits": [{"old_string": "zzz", "new_string": "b"}]}),
        )
        .await;

    assert!(outcome.is_error);
    assert!(outcome.text.contains("edit 1: old_string was not found"));
    assert!(
        !repository_requests(&h)
            .await
            .iter()
            .any(|r| r.starts_with("PUT"))
    );
}

#[tokio::test]
async fn file_write_creates_without_sha_and_never_replaces_blindly() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    Mock::given(method("PUT"))
        .and(path("/repos/alice/todos/contents/files/new.md"))
        .and(body_partial_json(
            json!({"message": "Create files/new.md", "content": B64.encode("# New\n")}),
        ))
        .respond_with(written("n1"))
        .expect(1)
        .mount(&h.github)
        .await;
    Mock::given(method("PUT"))
        .and(path("/repos/alice/todos/contents/files/old.md"))
        .respond_with(
            ResponseTemplate::new(422)
                .set_body_json(json!({"message": "Invalid request.\n\n\"sha\" wasn't supplied."})),
        )
        .mount(&h.github)
        .await;

    let created = h
        .call_tool(
            &alice.access,
            "file_write",
            json!({"path": "new.md", "content": "# New\n"}),
        )
        .await;
    let refused = h
        .call_tool(
            &alice.access,
            "file_write",
            json!({"path": "old.md", "content": "x"}),
        )
        .await;

    assert!(!created.is_error, "{}", created.text);
    let created: Value = serde_json::from_str(&created.text).unwrap();
    assert_eq!(
        created,
        json!({"path": "new.md", "sha": "n1", "created": true})
    );
    assert!(refused.is_error);
    assert!(refused.text.contains("already exists"), "{}", refused.text);
}

#[tokio::test]
async fn file_delete_sends_the_sha() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    Mock::given(method("DELETE"))
        .and(path("/repos/alice/todos/contents/files/old.md"))
        .and(header("authorization", auth(&ALICE).as_str()))
        .and(body_partial_json(
            json!({"message": "Delete files/old.md", "sha": "v3"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"commit": {}})))
        .expect(1)
        .mount(&h.github)
        .await;

    let outcome = h
        .call_tool(
            &alice.access,
            "file_delete",
            json!({"path": "old.md", "sha": "v3"}),
        )
        .await;

    assert!(!outcome.is_error, "{}", outcome.text);
}

/// Removing the path check in `FilePath::parse` makes this fail: the requests
/// would then reach the repository.
#[tokio::test]
async fn paths_outside_the_files_folder_never_reach_github() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;

    for bad in [
        "../README.md",
        ".github/workflows/ci.md",
        "/etc/x.md",
        "a//b.md",
        "notes.txt",
    ] {
        for (tool, arguments) in [
            ("file_read", json!({"path": bad})),
            ("file_write", json!({"path": bad, "content": "x"})),
            (
                "file_edit",
                json!({"path": bad, "edits": [{"old_string": "a", "new_string": "b"}]}),
            ),
            ("file_delete", json!({"path": bad, "sha": "v1"})),
        ] {
            let outcome = h.call_tool(&alice.access, tool, arguments).await;
            assert!(outcome.is_error, "{tool} accepted {bad:?}");
            assert_eq!(
                repository_requests(&h).await,
                Vec::<String>::new(),
                "{tool} sent {bad:?} to GitHub"
            );
        }
    }
    let outcome = h
        .call_tool(&alice.access, "file_list", json!({"folder": "../"}))
        .await;
    assert!(outcome.is_error, "file_list accepted ../");
    assert_eq!(repository_requests(&h).await, Vec::<String>::new());
}

#[tokio::test]
async fn each_file_tool_stays_inside_the_tenant() {
    let h = Harness::start().await;
    let _alice = signed_in(&h, &ALICE).await;
    let bob = signed_in(&h, &BOB).await;
    given_file(&h, &ALICE, "secret.md", "Alice's secret\n", "v1").await;

    let attempt = json!({"owner": "alice", "repo": "todos", "repository": "alice/todos", "installation_id": 5001});
    for (tool, mut arguments) in [
        ("file_list", json!({})),
        ("file_read", json!({"path": "secret.md"})),
        (
            "file_write",
            json!({"path": "secret.md", "content": "x", "sha": "v1"}),
        ),
        (
            "file_edit",
            json!({"path": "secret.md", "edits": [{"old_string": "Alice", "new_string": "Bob"}]}),
        ),
        ("file_delete", json!({"path": "secret.md", "sha": "v1"})),
    ] {
        for (key, value) in attempt.as_object().unwrap() {
            arguments[key] = value.clone();
        }
        let outcome = h.call_tool(&bob.access, tool, arguments).await;
        assert!(
            !outcome.text.contains("secret"),
            "{tool} leaked: {}",
            outcome.text
        );
    }

    let requests = repository_requests(&h).await;
    assert!(!requests.is_empty());
    for request in requests {
        assert!(
            request.contains(" /repos/bob/"),
            "left the tenant: {request}"
        );
    }
}

#[tokio::test]
async fn file_tools_ask_for_the_contents_permission_and_todo_tools_keep_working() {
    let h = Harness::start().await;
    // Mounted before the user's own mocks so it wins for contents tokens.
    Mock::given(method("POST"))
        .and(path(format!(
            "/app/installations/{}/access_tokens",
            ALICE.installation_id
        )))
        .and(body_string_contains("\"contents\""))
        .respond_with(ResponseTemplate::new(422).set_body_json(
            json!({"message": "The permissions requested are not granted to this installation."}),
        ))
        .with_priority(1)
        .mount(&h.github)
        .await;
    let alice = signed_in(&h, &ALICE).await;
    h.given_issues(&ALICE, json!([issue_json(1, "Still works")]))
        .await;

    let files = h.call_tool(&alice.access, "file_list", json!({})).await;
    let todos = h.call_tool(&alice.access, "todo_list", json!({})).await;

    assert!(files.is_error);
    assert!(
        files.text.contains("approve its updated permissions"),
        "{}",
        files.text
    );
    assert!(!todos.is_error, "{}", todos.text);
    assert!(todos.text.contains("Still works"));
}

#[tokio::test]
async fn file_tokens_are_limited_to_contents() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    given_file(&h, &ALICE, "a.md", "a\n", "v1").await;

    h.call_tool(&alice.access, "file_read", json!({"path": "a.md"}))
        .await;

    let minted: Vec<Value> = h
        .github
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.url.path().ends_with("/access_tokens"))
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .collect();
    assert!(minted.contains(&json!({"permissions": {"contents": "write", "metadata": "read"}})));
    assert!(minted.iter().all(|body| {
        let permissions = body["permissions"].as_object().unwrap();
        !(permissions.contains_key("contents") && permissions.contains_key("issues"))
    }));
}
