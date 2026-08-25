use super::*;
use axum::routing::{delete as delete_route, post};
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use tokio::net::TcpListener;
#[tokio::test]
async fn issue_comment_delete_resolve_and_unresolve_match_go_http_contracts() {
    let app = Router::new()
        .route(
            "/api/comments/comment-1",
            delete_route(|| async { axum::http::StatusCode::NO_CONTENT }),
        )
        .route(
            "/api/comments/comment-1/resolve",
            post(|| async {
                Json(serde_json::json!({"id":"comment-1","resolved_at":"2026-08-24T00:00:00Z"}))
            })
            .delete(|| async { Json(serde_json::json!({"id":"comment-1","resolved_at":null})) }),
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

    let delete = Cli::try_parse_from(["cordy", "issue", "comment", "delete", "comment-1"])
        .expect("delete CLI");
    let output = run_with_input(&delete, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("delete comment");
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, "Comment comment-1 deleted.\n");

    let resolve = Cli::try_parse_from(["cordy", "issue", "comment", "resolve", "comment-1"])
        .expect("resolve CLI");
    let output = run_with_input(&resolve, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("resolve comment");
    assert_eq!(output.stderr, "Comment comment-1 resolved.\n");
    assert!(
        serde_json::from_str::<Value>(&output.stdout).expect("resolved JSON")["resolved_at"]
            .is_string()
    );

    let unresolve = Cli::try_parse_from([
        "cordy",
        "issue",
        "comment",
        "unresolve",
        "comment-1",
        "--output",
        "table",
    ])
    .expect("unresolve CLI");
    let output = run_with_input(&unresolve, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("unresolve comment");
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, "Comment comment-1 unresolved.\n");
    task.abort();
}
