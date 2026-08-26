use super::cli_test_helpers::*;
use super::*;
use axum::http::HeaderMap;
use axum::routing::{get, put};
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
#[test]
fn issue_update_parser_matches_go_registry_flags() {
    let cli = Cli::try_parse_from([
        "cordy",
        "issue",
        "update",
        "CORD-18",
        "--title",
        "Updated",
        "--description",
        "one\\ntwo",
        "--status",
        "in_review",
        "--priority",
        "urgent",
        "--assignee-id",
        "11111111-1111-1111-1111-111111111111",
        "--project",
        "",
        "--start-date",
        "",
        "--due-date",
        "2026-08-31",
        "--parent",
        "",
        "--stage",
        "2",
        "--position",
        "1.5",
        "--no-start",
        "--output",
        "table",
    ])
    .expect("issue update CLI");
    let args = issue_update_args(&cli);
    assert_eq!(args.id, "CORD-18");
    assert_eq!(args.title.as_deref(), Some("Updated"));
    assert_eq!(args.description.as_deref(), Some("one\\ntwo"));
    assert_eq!(args.project.as_deref(), Some(""));
    assert_eq!(args.start_date.as_deref(), Some(""));
    assert_eq!(args.parent.as_deref(), Some(""));
    assert_eq!(args.stage, Some(2));
    assert_eq!(args.position, Some(1.5));
    assert!(args.no_start);
    assert_eq!(args.output, OutputFormat::Table);
}

#[tokio::test]
async fn issue_update_rejects_invalid_enums_before_client_creation() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let cli = Cli::try_parse_from(["cordy", "issue", "update", "CORD-18", "--priority", "P1"])
        .expect("update CLI");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("priority is rejected locally");
    assert!(error.to_string().contains("valid values"));
}

#[tokio::test]
async fn issue_update_resolves_references_and_puts_only_changed_fields() {
    let captured = Arc::new(Mutex::new(None::<Value>));
    let captured_by_update = Arc::clone(&captured);
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
        )
        .route(
            "/api/issues/PARENT-1",
            get(|| async { Json(serde_json::json!({"id":"parent-uuid","identifier":"CORD-1"})) }),
        )
        .route(
            "/api/projects",
            get(|| async { Json(serde_json::json!({"projects":[{"id":"abcd0000-0000-0000-0000-000000000000","title":"Migration","status":"active"}]})) }),
        )
        .route(
            "/api/workspaces/workspace-1/members",
            get(|| async { Json(serde_json::json!([{"user_id":"member-uuid","name":"Ada","email":"ada@example.com"}])) }),
        )
        .route("/api/agents", get(|| async { Json(serde_json::json!([])) }))
        .route("/api/squads", get(|| async { Json(serde_json::json!([])) }))
        .route(
            "/api/issues/issue-uuid",
            put(move |headers: HeaderMap, Json(body): Json<Value>| {
                let captured = Arc::clone(&captured_by_update);
                async move {
                    assert_eq!(headers["authorization"], "Bearer token-1");
                    *captured.lock().expect("capture update") = Some(body.clone());
                    Json(serde_json::json!({
                        "id":"issue-uuid","identifier":"CORD-18","title":body["title"],
                        "status":body["status"],"priority":body["priority"]
                    }))
                }
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");
    environment.set("CORDY_TOKEN", "token-1");
    let cli = Cli::try_parse_from([
        "cordy",
        "issue",
        "update",
        "CORD-18",
        "--title",
        "Updated",
        "--description",
        "one\\ntwo",
        "--status",
        "in_review",
        "--priority",
        "urgent",
        "--assignee",
        "Ada",
        "--project",
        "abcd",
        "--start-date",
        "",
        "--due-date",
        "2026-08-31",
        "--parent",
        "PARENT-1",
        "--stage",
        "2",
        "--position",
        "1.5",
        "--no-start",
        "--output",
        "table",
    ])
    .expect("update CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("update issue");
    assert!(output.stdout.starts_with("KEY"));
    assert!(output.stdout.contains("CORD-18"));
    let body = captured
        .lock()
        .expect("body")
        .clone()
        .expect("captured body");
    assert_eq!(body["title"], "Updated");
    assert_eq!(body["description"], "one\ntwo");
    assert_eq!(body["status"], "in_review");
    assert_eq!(body["priority"], "urgent");
    assert_eq!(body["assignee_type"], "member");
    assert_eq!(body["assignee_id"], "member-uuid");
    assert_eq!(body["project_id"], "abcd0000-0000-0000-0000-000000000000");
    assert_eq!(body["start_date"], "");
    assert_eq!(body["due_date"], "2026-08-31");
    assert_eq!(body["parent_issue_id"], "parent-uuid");
    assert_eq!(body["stage"], 2);
    assert_eq!(body["position"], 1.5);
    assert_eq!(body["suppress_run"], true);
    task.abort();
}

#[tokio::test]
async fn issue_update_supports_explicit_clears_and_rejects_no_changes() {
    let captured = Arc::new(Mutex::new(None::<Value>));
    let captured_by_update = Arc::clone(&captured);
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
        )
        .route(
            "/api/issues/issue-uuid",
            put(move |Json(body): Json<Value>| {
                let captured = Arc::clone(&captured_by_update);
                async move {
                    *captured.lock().expect("capture update") = Some(body);
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");
    environment.set("CORDY_TOKEN", "token-1");

    let clear = Cli::try_parse_from([
        "cordy",
        "issue",
        "update",
        "CORD-18",
        "--description",
        "",
        "--project",
        "",
        "--parent",
        "",
    ])
    .expect("clear CLI");
    run_with_input(&clear, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("clear fields");
    let body = captured
        .lock()
        .expect("body")
        .clone()
        .expect("captured body");
    assert_eq!(body["description"], "");
    assert_eq!(body["project_id"], Value::Null);
    assert_eq!(body["parent_issue_id"], Value::Null);

    let no_changes =
        Cli::try_parse_from(["cordy", "issue", "update", "CORD-18"]).expect("no changes CLI");
    let error = run_with_input(
        &no_changes,
        &environment,
        &mut Cursor::new(Vec::<u8>::new()),
    )
    .await
    .expect_err("no fields");
    assert!(error.to_string().contains("no fields to update"));
    task.abort();
}
