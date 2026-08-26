use super::cli_test_helpers::*;
use super::*;
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use std::fs;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
#[test]
fn issue_comment_add_parser_and_content_sources_match_go() {
    let cli = Cli::try_parse_from([
        "cordy",
        "issue",
        "comment",
        "add",
        "CORD-18",
        "--content",
        "one\\ntwo",
        "--parent",
        "comment-1",
        "--attachment",
        "one.png",
        "--output",
        "table",
    ])
    .expect("comment add CLI");
    let args = issue_comment_add_args(&cli);
    assert_eq!(args.issue_id, "CORD-18");
    assert_eq!(args.parent.as_deref(), Some("comment-1"));
    assert_eq!(args.attachment, vec![String::from("one.png")]);
    assert_eq!(args.output, OutputFormat::Table);
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    assert_eq!(
        resolve_issue_comment_content(args, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .expect("inline content"),
        Some("one\ntwo".into())
    );

    let empty_file = Cli::try_parse_from([
        "cordy",
        "issue",
        "comment",
        "add",
        "CORD-18",
        "--content-file",
        "",
    ])
    .expect("empty file reaches runtime");
    assert!(resolve_issue_comment_content(
        issue_comment_add_args(&empty_file),
        &environment,
        &mut Cursor::new(Vec::<u8>::new())
    )
    .expect("empty file is unset")
    .is_none());
}

#[tokio::test]
async fn issue_comment_add_prevalidates_uploads_then_posts_attachment_ids() {
    let captured = Arc::new(Mutex::new(None::<Value>));
    let captured_by_comment = Arc::clone(&captured);
    let uploads = Arc::new(Mutex::new(0_usize));
    let uploads_by_handler = Arc::clone(&uploads);
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
        )
        .route(
            "/api/upload-file",
            post(move |headers: HeaderMap, _body: axum::body::Bytes| {
                let uploads = Arc::clone(&uploads_by_handler);
                async move {
                    *uploads.lock().expect("uploads") += 1;
                    assert!(headers["content-type"]
                        .to_str()
                        .expect("content type")
                        .starts_with("multipart/form-data; boundary="));
                    Json(serde_json::json!({"id":"attachment-1"}))
                }
            }),
        )
        .route(
            "/api/issues/issue-uuid/comments",
            post(move |Json(body): Json<Value>| {
                let captured = Arc::clone(&captured_by_comment);
                async move {
                    *captured.lock().expect("comment body") = Some(body.clone());
                    Json(serde_json::json!({"id":"comment-1","content":body["content"]}))
                }
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    fs::write(cwd.path().join("proof.txt"), b"proof").expect("attachment");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");
    environment.set("CORDY_TOKEN", "token-1");
    let cli = Cli::try_parse_from([
        "cordy",
        "issue",
        "comment",
        "add",
        "CORD-18",
        "--content",
        "Completed\\nSee proof.",
        "--parent",
        "parent-comment",
        "--attachment",
        "proof.txt",
    ])
    .expect("comment add CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("add comment");
    assert!(output.stderr.contains("Uploaded proof.txt"));
    assert!(output.stderr.contains("Comment added to issue CORD-18."));
    assert_eq!(*uploads.lock().expect("uploads"), 1);
    let body = captured
        .lock()
        .expect("body")
        .clone()
        .expect("captured body");
    assert_eq!(body["content"], "Completed\nSee proof.");
    assert_eq!(body["parent_id"], "parent-comment");
    assert_eq!(body["attachment_ids"], serde_json::json!(["attachment-1"]));
    task.abort();
}

#[tokio::test]
async fn issue_comment_add_rejects_missing_content_before_network() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let cli = Cli::try_parse_from(["cordy", "issue", "comment", "add", "CORD-18"])
        .expect("missing content reaches runtime");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("missing content");
    assert!(error.to_string().contains("--content-file is required"));
}
