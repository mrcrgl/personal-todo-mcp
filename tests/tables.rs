//! Table tools over the real MCP endpoint. As in `tenancy.rs`, the fake GitHub
//! answers a repository only for its owner's installation token.

mod support;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value, json};
use support::*;
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

const HEADER: &str = concat!(
    r#"{"version":1,"next_id":3,"columns":["#,
    r#"{"name":"title","type":"string","required":true},"#,
    r#"{"name":"status","type":"enum","values":["unread","read"]},"#,
    r#"{"name":"pages","type":"integer"}]}"#,
);
const ROW_1: &str = r#"{"id":1,"title":"Dune","status":"read","pages":600}"#;
const ROW_2: &str = r#"{"id":2,"title":"Emma","pages":400}"#;

fn books() -> String {
    format!("{HEADER}\n{ROW_1}\n{ROW_2}\n")
}

fn auth(user: &FakeUser) -> String {
    format!("Bearer {}", user.installation_token())
}

fn table_path(user: &FakeUser, name: &str) -> String {
    format!("/repos/{}/todos/contents/tables/{name}.jsonl", user.login)
}

/// A table in a user's repository, readable only with that user's token.
async fn given_table(h: &Harness, user: &FakeUser, name: &str, text: &str, sha: &str) {
    Mock::given(method("GET"))
        .and(path(table_path(user, name)))
        .and(header("authorization", auth(user).as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "type": "file",
            "encoding": "base64",
            "size": text.len(),
            "sha": sha,
            "content": B64.encode(text),
        })))
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

/// The text of every table written, decoded.
async fn written_tables(h: &Harness) -> Vec<String> {
    h.github
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.method.as_str() == "PUT")
        .map(|request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let bytes = B64.decode(body["content"].as_str().unwrap()).unwrap();
            String::from_utf8(bytes).unwrap()
        })
        .collect()
}

async fn signed_in(h: &Harness, user: &FakeUser) -> Tokens {
    h.given_user(user).await;
    h.sign_in(user).await
}

fn parsed(outcome: &ToolOutcome) -> Value {
    assert!(!outcome.is_error, "{}", outcome.text);
    serde_json::from_str(&outcome.text).unwrap()
}

#[tokio::test]
async fn table_create_writes_the_schema_without_a_sha() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    Mock::given(method("PUT"))
        .and(path(table_path(&ALICE, "books")))
        .and(header("authorization", auth(&ALICE).as_str()))
        .and(body_partial_json(
            json!({"message": "Create tables/books.jsonl"}),
        ))
        .respond_with(written("t1"))
        .expect(1)
        .mount(&h.github)
        .await;

    let outcome = h
        .call_tool(
            &alice.access,
            "table_create",
            json!({
                "name": "books",
                "description": "What I read",
                "columns": [
                    {"name": "title", "type": "string", "required": true},
                    {"name": "status", "type": "enum", "values": ["unread", "read"]},
                ],
            }),
        )
        .await;

    let created = parsed(&outcome);
    assert_eq!(created["sha"], "t1");
    assert_eq!(created["rows"], 0);
    assert_eq!(created["columns"][1]["values"], json!(["unread", "read"]));
    let body: Value = serde_json::from_slice(
        &h.github
            .received_requests()
            .await
            .unwrap()
            .last()
            .unwrap()
            .body,
    )
    .unwrap();
    assert!(
        body.get("sha").is_none(),
        "a create must not name a version"
    );
    assert_eq!(
        written_tables(&h).await,
        [concat!(
            r#"{"version":1,"description":"What I read","next_id":1,"columns":["#,
            r#"{"name":"title","type":"string","required":true},"#,
            r#"{"name":"status","type":"enum","values":["unread","read"]}]}"#,
            "\n"
        )]
    );
}

#[tokio::test]
async fn table_create_never_replaces_an_existing_table() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    Mock::given(method("PUT"))
        .and(path(table_path(&ALICE, "books")))
        .respond_with(
            ResponseTemplate::new(422)
                .set_body_json(json!({"message": "Invalid request.\n\n\"sha\" wasn't supplied."})),
        )
        .mount(&h.github)
        .await;

    let outcome = h
        .call_tool(
            &alice.access,
            "table_create",
            json!({"name": "books", "columns": [{"name": "title", "type": "string"}]}),
        )
        .await;

    assert!(outcome.is_error);
    assert!(outcome.text.contains("already exists"), "{}", outcome.text);
}

#[tokio::test]
async fn table_insert_writes_a_batch_in_one_commit_against_the_sha_it_read() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    given_table(&h, &ALICE, "books", &books(), "v1").await;
    Mock::given(method("PUT"))
        .and(path(table_path(&ALICE, "books")))
        .and(header("authorization", auth(&ALICE).as_str()))
        .and(body_partial_json(
            json!({"message": "Update tables/books.jsonl", "sha": "v1"}),
        ))
        .respond_with(written("v2"))
        .expect(1)
        .mount(&h.github)
        .await;

    let outcome = h
        .call_tool(
            &alice.access,
            "table_insert",
            json!({"name": "books", "rows": [
                {"title": "Kim", "pages": 300},
                {"pages": 150, "title": "Ubik", "status": "unread"},
            ]}),
        )
        .await;

    assert_eq!(
        parsed(&outcome),
        json!({"sha": "v2", "inserted": 2, "ids": [3, 4], "total_rows": 4})
    );
    let expected = format!(
        "{}\n{ROW_1}\n{ROW_2}\n{}\n{}\n",
        HEADER.replace(r#""next_id":3"#, r#""next_id":5"#),
        r#"{"id":3,"title":"Kim","pages":300}"#,
        r#"{"id":4,"title":"Ubik","status":"unread","pages":150}"#,
    );
    assert_eq!(written_tables(&h).await, [expected]);
}

#[tokio::test]
async fn a_row_that_breaks_the_schema_writes_nothing() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    given_table(&h, &ALICE, "books", &books(), "v1").await;

    let outcome = h
        .call_tool(
            &alice.access,
            "table_insert",
            json!({"name": "books", "rows": [
                {"title": "Kim"},
                {"title": "Secret title", "status": "lost"},
            ]}),
        )
        .await;

    assert!(outcome.is_error);
    assert_eq!(
        outcome.text,
        "item 2, column status: not one of the column's values"
    );
    assert!(written_tables(&h).await.is_empty());
}

#[tokio::test]
async fn table_query_filters_sorts_and_pages() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    given_table(&h, &ALICE, "books", &books(), "v1").await;

    let outcome = h
        .call_tool(
            &alice.access,
            "table_query",
            json!({
                "name": "books",
                "where": [{"column": "pages", "op": "gte", "value": 100}],
                "order_by": [{"column": "pages"}],
                "limit": 1,
            }),
        )
        .await;

    assert_eq!(
        parsed(&outcome),
        json!({
            "sha": "v1",
            "total_rows": 2,
            "matched": 2,
            "offset": 0,
            "next_offset": 1,
            "rows": [{"id": 2, "title": "Emma", "pages": 400}],
        })
    );
    assert!(
        outcome
            .text
            .contains(r#"{"id":2,"title":"Emma","pages":400}"#),
        "rows keep column order: {}",
        outcome.text
    );
}

#[tokio::test]
async fn table_update_where_changes_matching_rows() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    given_table(&h, &ALICE, "books", &books(), "v1").await;
    Mock::given(method("PUT"))
        .and(path(table_path(&ALICE, "books")))
        .respond_with(written("v2"))
        .expect(1)
        .mount(&h.github)
        .await;

    let outcome = h
        .call_tool(
            &alice.access,
            "table_update",
            json!({
                "name": "books",
                "where": [{"column": "status", "op": "is_null"}],
                "set": {"status": "unread"},
            }),
        )
        .await;

    assert_eq!(
        parsed(&outcome),
        json!({"sha": "v2", "updated": 1, "total_rows": 2})
    );
    assert!(
        written_tables(&h).await[0]
            .contains(r#"{"id":2,"title":"Emma","status":"unread","pages":400}"#)
    );
}

#[tokio::test]
async fn a_change_reports_a_conflict_after_one_retry() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    given_table(&h, &ALICE, "books", &books(), "v1").await;
    Mock::given(method("PUT"))
        .and(path(table_path(&ALICE, "books")))
        .respond_with(
            ResponseTemplate::new(409)
                .set_body_json(json!({"message": "tables/books.jsonl does not match v1"})),
        )
        .expect(2)
        .mount(&h.github)
        .await;

    let outcome = h
        .call_tool(
            &alice.access,
            "table_delete",
            json!({"name": "books", "ids": [1]}),
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
async fn a_damaged_table_names_the_line() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    let damaged = books().replace(r#""pages":400"#, r#""pages":"many""#);
    given_table(&h, &ALICE, "books", &damaged, "v1").await;

    let outcome = h
        .call_tool(&alice.access, "table_query", json!({"name": "books"}))
        .await;

    assert!(outcome.is_error);
    assert_eq!(
        outcome.text,
        "the stored table is damaged at line 3: row 2, column pages: expected an integer. \
         Fix that line in the repository, or drop the table"
    );
}

#[tokio::test]
async fn table_list_reads_the_default_branch_tree() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    Mock::given(method("GET"))
        .and(path("/repos/alice/todos/git/trees/main"))
        .and(query_param("recursive", "1"))
        .and(header("authorization", auth(&ALICE).as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "truncated": false,
            "tree": [
                {"path": "files/a.md", "type": "blob", "size": 1},
                {"path": "tables/wine.jsonl", "type": "blob", "size": 9},
                {"path": "tables/books.jsonl", "type": "blob", "size": 5},
                {"path": "tables/notes.md", "type": "blob", "size": 2},
            ],
        })))
        .mount(&h.github)
        .await;

    let outcome = h.call_tool(&alice.access, "table_list", json!({})).await;

    assert_eq!(
        parsed(&outcome),
        json!({"tables": [{"name": "books", "size": 5}, {"name": "wine", "size": 9}], "truncated": false})
    );
}

#[tokio::test]
async fn table_drop_sends_the_sha() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    Mock::given(method("DELETE"))
        .and(path(table_path(&ALICE, "books")))
        .and(header("authorization", auth(&ALICE).as_str()))
        .and(body_partial_json(
            json!({"message": "Delete tables/books.jsonl", "sha": "v3"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"commit": {}})))
        .expect(1)
        .mount(&h.github)
        .await;

    let outcome = h
        .call_tool(
            &alice.access,
            "table_drop",
            json!({"name": "books", "sha": "v3"}),
        )
        .await;

    assert_eq!(parsed(&outcome), json!({"dropped": true}));
}

/// Removing the check in `TableName::parse` makes this fail: the requests
/// would then reach the repository.
#[tokio::test]
async fn table_names_that_could_leave_the_tables_folder_never_reach_github() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;

    for bad in ["../README", "a/b", ".github", "books.jsonl", "Books", ""] {
        for (tool, arguments) in [
            (
                "table_create",
                json!({"name": bad, "columns": [{"name": "a", "type": "string"}]}),
            ),
            ("table_describe", json!({"name": bad})),
            ("table_alter", json!({"name": bad, "remove": ["a"]})),
            ("table_drop", json!({"name": bad, "sha": "v1"})),
            ("table_query", json!({"name": bad})),
            ("table_insert", json!({"name": bad, "rows": [{"a": "x"}]})),
            (
                "table_update",
                json!({"name": bad, "rows": [{"id": 1, "set": {"a": "x"}}]}),
            ),
            ("table_delete", json!({"name": bad, "ids": [1]})),
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
}

#[tokio::test]
async fn each_table_tool_stays_inside_the_tenant() {
    let h = Harness::start().await;
    let _alice = signed_in(&h, &ALICE).await;
    let bob = signed_in(&h, &BOB).await;
    given_table(&h, &ALICE, "books", &books(), "v1").await;

    let attempt = json!({"owner": "alice", "repo": "todos", "repository": "alice/todos", "installation_id": 5001});
    for (tool, mut arguments) in [
        ("table_list", json!({})),
        (
            "table_create",
            json!({"name": "books", "columns": [{"name": "title", "type": "string"}]}),
        ),
        ("table_describe", json!({"name": "books"})),
        (
            "table_alter",
            json!({"name": "books", "add": [{"name": "rating", "type": "integer"}]}),
        ),
        ("table_drop", json!({"name": "books", "sha": "v1"})),
        ("table_query", json!({"name": "books"})),
        (
            "table_insert",
            json!({"name": "books", "rows": [{"title": "x"}]}),
        ),
        (
            "table_update",
            json!({"name": "books", "rows": [{"id": 1, "set": {"title": "x"}}]}),
        ),
        ("table_delete", json!({"name": "books", "ids": [1]})),
    ] {
        for (key, value) in attempt.as_object().unwrap() {
            arguments[key] = value.clone();
        }
        let outcome = h.call_tool(&bob.access, tool, arguments).await;
        for secret in ["Dune", "Emma"] {
            assert!(
                !outcome.text.contains(secret),
                "{tool} leaked: {}",
                outcome.text
            );
        }
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
async fn table_tokens_are_limited_to_contents() {
    let h = Harness::start().await;
    let alice = signed_in(&h, &ALICE).await;
    given_table(&h, &ALICE, "books", &books(), "v1").await;

    h.call_tool(&alice.access, "table_describe", json!({"name": "books"}))
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
}
