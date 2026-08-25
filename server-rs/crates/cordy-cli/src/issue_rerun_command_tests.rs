use super::*;
use super::cli_test_helpers::*;
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use tokio::net::TcpListener;

#[tokio::test]
async fn issue_rerun_posts_fresh_task_and_formats_agent_name() {
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
        )
        .route(
            "/api/issues/issue-uuid/rerun",
            post(|Json(body): Json<Value>| async move {
                assert_eq!(body, serde_json::json!({}));
                Json(serde_json::json!({"id":"task-1","agent_id":"agent-1","status":"queued"}))
            }),
        )
        .route(
            "/api/agents",
            get(|| async { Json(vec![serde_json::json!({"id":"agent-1","name":"CodeBot"})]) }),
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
    let cli = Cli::try_parse_from(["cordy", "issue", "rerun", "CORD-18", "--output", "table"])
        .expect("rerun CLI");
    assert_eq!(issue_rerun_args(&cli).issue_id, "CORD-18");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("rerun issue");
    assert_eq!(output.stdout, "Re-enqueued task task-1 on agent CodeBot\n");
    assert!(output.stderr.is_empty());
    task.abort();
}
