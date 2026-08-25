use super::*;
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use std::collections::HashMap;
use std::io::Cursor;
use tokio::net::TcpListener;
#[test]
fn issue_runs_parser_and_table_match_go_contract() {
    let cli = Cli::try_parse_from([
        "cordy",
        "issue",
        "runs",
        "CORD-18",
        "--full-id",
        "--output",
        "json",
    ])
    .expect("runs CLI");
    let args = issue_runs_args(&cli);
    assert_eq!(args.issue_id, "CORD-18");
    assert!(args.full_id);
    assert_eq!(args.output, OutputFormat::Json);

    let runs = vec![serde_json::json!({
        "id":"11111111-1111-1111-1111-111111111111","agent_id":"agent-1",
        "status":"failed","started_at":"2026-08-24T12:34:56Z",
        "completed_at":"2026-08-24T12:40:00Z","error":"x".repeat(51)
    })];
    let actors = IssueActorNames(HashMap::from([("agent:agent-1".into(), "CodeBot".into())]));
    let short = format_issue_runs_table(&runs, false, &actors);
    assert!(short.contains("11111111"));
    assert!(!short.contains("11111111-1111"));
    assert!(short.contains("CodeBot"));
    assert!(short.contains("2026-08-24T12:34"));
    assert!(short.contains("xxx..."));
    let full = format_issue_runs_table(&runs, true, &actors);
    assert!(full.contains("11111111-1111-1111-1111-111111111111"));
}

#[tokio::test]
async fn issue_runs_resolves_issue_fetches_task_runs_and_actor_names() {
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
        )
        .route(
            "/api/issues/issue-uuid/task-runs",
            get(|| async {
                Json(vec![serde_json::json!({
                    "id":"task-uuid","agent_id":"agent-1","status":"completed",
                    "started_at":"2026-08-24T12:34:56Z","completed_at":"2026-08-24T12:40:00Z"
                })])
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
    let cli = Cli::try_parse_from(["cordy", "issue", "runs", "CORD-18"]).expect("runs CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list runs");
    assert!(output.stdout.starts_with("ID"));
    assert!(output.stdout.contains("CodeBot"));
    assert!(output.stdout.contains("completed"));
    task.abort();
}
