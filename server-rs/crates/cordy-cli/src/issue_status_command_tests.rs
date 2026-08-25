use super::*;
use super::cli_test_helpers::*;
use axum::routing::{get, put};
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
#[test]
fn issue_status_parser_matches_go_registry_flags() {
    let cli = Cli::try_parse_from([
        "cordy",
        "issue",
        "status",
        "CORD-18",
        "custom_status",
        "--no-start",
        "--output",
        "json",
    ])
    .expect("status CLI");
    let args = issue_status_args(&cli);
    assert_eq!(args.id, "CORD-18");
    assert_eq!(args.status, "custom_status");
    assert!(args.no_start);
    assert_eq!(args.output, OutputFormat::Json);
}

#[tokio::test]
async fn issue_status_validates_then_puts_status_and_suppress_run() {
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
                    *captured.lock().expect("capture status") = Some(body);
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18","status":"custom_status"}))
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
        "status",
        "CORD-18",
        "custom_status",
        "--no-start",
        "--output",
        "json",
    ])
    .expect("status CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("status update");
    assert_eq!(
        output.stderr,
        "Issue CORD-18 status changed to custom_status.\n"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&output.stdout).expect("status JSON")["status"],
        "custom_status"
    );
    let body = captured
        .lock()
        .expect("body")
        .clone()
        .expect("captured body");
    assert_eq!(body["status"], "custom_status");
    assert_eq!(body["suppress_run"], true);
    task.abort();
}

#[tokio::test]
async fn issue_status_rejects_malformed_status_before_network() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let cli = Cli::try_parse_from(["cordy", "issue", "status", "CORD-18", "not a status"])
        .expect("status CLI");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("malformed status");
    assert!(error.to_string().contains("status key"));
}
