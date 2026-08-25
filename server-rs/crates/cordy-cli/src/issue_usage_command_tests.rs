use super::*;
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use tokio::net::TcpListener;
#[test]
fn issue_usage_parser_and_number_format_match_go() {
    let cli = Cli::try_parse_from(["cordy", "issue", "usage", "CORD-18", "--output", "json"])
        .expect("usage CLI");
    let args = issue_usage_args(&cli);
    assert_eq!(args.issue_id, "CORD-18");
    assert_eq!(args.output, OutputFormat::Json);
    assert_eq!(format_metadata_value(Some(&serde_json::json!(42.0))), "42");
    assert_eq!(
        format_metadata_value(Some(&serde_json::json!(1234567890123_u64))),
        "1234567890123"
    );
    assert_eq!(format_metadata_value(None), "null");
}

#[tokio::test]
async fn issue_usage_resolves_issue_and_renders_aggregate_table() {
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
        )
        .route(
            "/api/issues/issue-uuid/usage",
            get(|| async {
                Json(serde_json::json!({
                    "total_input_tokens":1000,"total_output_tokens":200,
                    "total_cache_read_tokens":300,"total_cache_write_tokens":40,"task_count":2
                }))
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
    let cli = Cli::try_parse_from(["cordy", "issue", "usage", "CORD-18"]).expect("usage CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("issue usage");
    assert!(output.stdout.starts_with("INPUT_TOKENS"));
    assert!(output.stdout.contains("1000"));
    assert!(output.stdout.contains("300"));
    assert!(output.stdout.contains("2"));
    task.abort();
}
