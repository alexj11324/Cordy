use super::*;
use super::cli_test_helpers::*;
use axum::routing::{get, put};
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
#[tokio::test]
async fn issue_assign_parser_and_local_validation_match_go() {
    let cli = Cli::try_parse_from([
        "cordy",
        "issue",
        "assign",
        "CORD-18",
        "--to-id",
        "11111111-1111-1111-1111-111111111111",
        "--no-start",
        "--output",
        "table",
    ])
    .expect("assign CLI");
    let args = issue_assign_args(&cli);
    assert_eq!(args.id, "CORD-18");
    assert_eq!(
        args.to_id.as_deref(),
        Some("11111111-1111-1111-1111-111111111111")
    );
    assert!(args.no_start);
    assert_eq!(args.output, OutputFormat::Table);

    let missing = Cli::try_parse_from(["cordy", "issue", "assign", "CORD-18"])
        .expect("validation is at runtime");
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let error = run_with_input(&missing, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("missing target");
    assert!(error.to_string().contains("provide --to"));
}

#[tokio::test]
async fn issue_assign_puts_resolved_actor_and_supports_unassign() {
    let bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
    let bodies_by_update = Arc::clone(&bodies);
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
        )
        .route(
            "/api/workspaces/workspace-1/members",
            get(|| async { Json(serde_json::json!([])) }),
        )
        .route(
            "/api/agents",
            get(|| async { Json(serde_json::json!([{"id":"11111111-1111-1111-1111-111111111111","name":"CodeBot"}])) }),
        )
        .route("/api/squads", get(|| async { Json(serde_json::json!([])) }))
        .route(
            "/api/issues/issue-uuid",
            put(move |Json(body): Json<Value>| {
                let bodies = Arc::clone(&bodies_by_update);
                async move {
                    bodies.lock().expect("bodies").push(body);
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

    let assign = Cli::try_parse_from([
        "cordy",
        "issue",
        "assign",
        "CORD-18",
        "--to-id",
        "11111111-1111-1111-1111-111111111111",
        "--no-start",
    ])
    .expect("assign CLI");
    let output = run_with_input(&assign, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("assign");
    assert!(output.stderr.contains("assigned to agent:CodeBot"));
    let assign_body = bodies.lock().expect("bodies")[0].clone();
    assert_eq!(assign_body["assignee_type"], "agent");
    assert_eq!(
        assign_body["assignee_id"],
        "11111111-1111-1111-1111-111111111111"
    );
    assert_eq!(assign_body["suppress_run"], true);

    let unassign = Cli::try_parse_from([
        "cordy",
        "issue",
        "assign",
        "CORD-18",
        "--unassign",
        "--output",
        "table",
    ])
    .expect("unassign CLI");
    let output = run_with_input(&unassign, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("unassign");
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, "Issue CORD-18 unassigned.\n");
    let unassign_body = bodies.lock().expect("bodies")[1].clone();
    assert_eq!(unassign_body["assignee_type"], Value::Null);
    assert_eq!(unassign_body["assignee_id"], Value::Null);
    task.abort();
}

#[tokio::test]
async fn issue_assign_rejects_no_start_with_unassign_before_network() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let cli = Cli::try_parse_from([
        "cordy",
        "issue",
        "assign",
        "CORD-18",
        "--unassign",
        "--no-start",
    ])
    .expect("assign CLI");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("invalid no-start unassign");
    assert!(error.to_string().contains("--no-start"));
}
