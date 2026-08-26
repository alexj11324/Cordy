use super::cli_test_helpers::*;
use super::*;
use axum::extract::Request;
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use std::collections::HashMap;
use std::io::Cursor;
use tokio::net::TcpListener;
#[tokio::test]
async fn issue_comment_list_parser_and_validation_match_go() {
    let cli = Cli::try_parse_from([
        "cordy",
        "issue",
        "comment",
        "list",
        "CORD-18",
        "--thread",
        "comment-1",
        "--tail",
        "0",
        "--summary",
        "--compact",
        "--full",
        "--before",
        "2026-08-24T00:00:00Z",
        "--before-id",
        "comment-2",
        "--output",
        "json",
    ])
    .expect("comment list CLI");
    let args = issue_comment_list_args(&cli);
    assert_eq!(args.thread.as_deref(), Some("comment-1"));
    assert_eq!(args.tail, Some(0));
    assert!(args.summary && args.compact && args.full);
    assert_eq!(args.output, OutputFormat::Json);

    let invalid = Cli::try_parse_from([
        "cordy", "issue", "comment", "list", "CORD-18", "--tail", "1",
    ])
    .expect("combination validation is at runtime");
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let error = run_with_input(&invalid, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("tail requires thread");
    assert!(error.to_string().contains("--tail requires --thread"));
}

#[tokio::test]
async fn issue_comment_list_sends_folded_recent_query_surfaces_cursor_and_compacts_json() {
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
        )
        .route(
            "/api/issues/issue-uuid/comments",
            get(|request: Request| async move {
                let query = request.uri().query().unwrap_or_default();
                assert!(query.contains("summary=true"));
                assert!(query.contains("fold=true"));
                assert!(query.contains("recent=2"));
                assert!(query.contains("before=2026-08-24T00%3A00%3A00Z"));
                assert!(query.contains("before_id=comment-2"));
                let mut headers = HeaderMap::new();
                headers.insert(
                    "X-Cordy-Next-Before",
                    "2026-08-23T23:00:00Z".parse().expect("cursor"),
                );
                headers.insert(
                    "X-Cordy-Next-Before-Id",
                    "comment-older".parse().expect("cursor id"),
                );
                (
                    headers,
                    Json(vec![serde_json::json!({
                        "id":"comment-1","issue_id":"issue-uuid","source_task_id":null,
                        "author_type":"member","author_id":"member-1","type":"comment",
                        "content":"summary","created_at":"2026-08-24T00:00:00Z",
                        "updated_at":"2026-08-24T00:00:00Z","parent_id":null,
                        "attachments":[]
                    })]),
                )
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
        "comment",
        "list",
        "CORD-18",
        "--recent",
        "2",
        "--summary",
        "--compact",
        "--before",
        "2026-08-24T00:00:00Z",
        "--before-id",
        "comment-2",
        "--output",
        "json",
    ])
    .expect("comment list CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list comments");
    assert_eq!(
        output.stderr,
        "Next thread cursor: --before 2026-08-23T23:00:00Z --before-id comment-older\n"
    );
    let comments: Value = serde_json::from_str(&output.stdout).expect("comments JSON");
    let comment = &comments[0];
    assert!(comment.get("issue_id").is_none());
    assert!(comment.get("source_task_id").is_none());
    assert!(comment.get("updated_at").is_none());
    assert!(comment.get("parent_id").is_none());
    assert!(comment.get("attachments").is_none());
    task.abort();
}

#[test]
fn issue_comment_list_table_truncates_and_formats_actor_fallback() {
    let comments = vec![serde_json::json!({
        "id":"comment-1","parent_id":null,"author_type":"agent","author_id":"agent-1",
        "type":"comment","content":"x".repeat(81),"created_at":"2026-08-24T12:34:56Z"
    })];
    let actors = IssueActorNames(HashMap::from([("agent:agent-1".into(), "CodeBot".into())]));
    let table = format_issue_comments_table(&comments, &actors);
    assert!(table.starts_with("ID"));
    assert!(table.contains("agent:CodeBot"));
    assert!(table.contains("2026-08-24T12:34"));
    assert!(table.contains("xxx..."));
}
