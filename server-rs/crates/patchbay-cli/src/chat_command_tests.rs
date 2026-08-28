use super::*;
use axum::extract::Request;
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use tokio::net::TcpListener;

#[tokio::test]
async fn chat_history_and_thread_match_go_query_and_render_contracts() {
    let app = Router::new()
        .route(
            "/api/chat/history",
            get(|request: Request| async move {
                assert_eq!(request.uri().query(), Some("before=cursor%2Fone&limit=25"));
                Json(serde_json::json!({
                    "messages":[{
                        "ts":"2026-08-24T00:00:00Z","role":"user","author":"Ada",
                        "thread_id":"thread/1","reply_count":2,"text":"status?"
                    }],"next_cursor":"older"
                }))
            }),
        )
        .route(
            "/api/chat/thread",
            get(|request: Request| async move {
                assert_eq!(request.uri().query(), Some("id=thread%2F1"));
                Json(serde_json::json!({"note":"thread is unavailable"}))
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");

    let history = Cli::try_parse_from([
        "patchbay",
        "chat",
        "history",
        "--before",
        "cursor/one",
        "--limit",
        "25",
        "--output",
        "table",
    ])
    .expect("chat history CLI");
    let history = run_with_input(&history, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("chat history");
    assert!(history.stdout.starts_with("TS"));
    assert!(history.stdout.contains("thread/1"));
    assert!(history.stdout.contains("2"));
    assert!(history.stdout.contains("status?"));

    let thread = Cli::try_parse_from([
        "patchbay", "chat", "thread", "thread/1", "--output", "table",
    ])
    .expect("chat thread CLI");
    let thread = run_with_input(&thread, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("chat thread");
    assert_eq!(thread.stdout, "thread is unavailable\n");
    server.abort();
}
