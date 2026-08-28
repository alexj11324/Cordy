use super::*;
use axum::extract::Request;
use axum::routing::{delete as delete_route, get, post};
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use tokio::net::TcpListener;

#[test]
fn issue_label_parser_and_table_match_go_contract() {
    let cli = Cli::try_parse_from([
        "patchbay",
        "issue",
        "label",
        "add",
        "CORD-18",
        "abcd",
        "--full-id",
        "--output",
        "json",
    ])
    .expect("issue label add CLI");
    let Command::Issue(IssueArgs {
        command:
            IssueCommand::Label(IssueLabelArgs {
                command: IssueLabelCommand::Add(args),
            }),
    }) = &cli.command
    else {
        panic!("expected issue label add");
    };
    assert_eq!(args.issue_id, "CORD-18");
    assert_eq!(args.label_id, "abcd");
    assert!(args.full_id);
    assert_eq!(args.output, OutputFormat::Json);

    let labels = [serde_json::json!({
        "id":"11111111-1111-1111-1111-111111111111","name":"Bug","color":"#ff0000"
    })];
    let short = format_label_table(&labels, false);
    assert!(short.starts_with("ID"));
    assert!(short.contains("11111111"));
    assert!(!short.contains("11111111-1111"));
    assert!(short.contains("Bug"));
    let full = format_label_table(&labels, true);
    assert!(full.contains("11111111-1111-1111-1111-111111111111"));
}

#[tokio::test]
async fn issue_label_add_resolves_prefix_and_returns_response_labels() {
    let label_id = "abcd1234-0000-0000-0000-000000000000";
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
        )
        .route(
            "/api/labels",
            get(move |request: Request| async move {
                assert_eq!(request.uri().query(), Some("workspace_id=workspace-1"));
                Json(serde_json::json!({
                    "labels":[{"id":label_id,"name":"Bug","color":"#ff0000"}]
                }))
            }),
        )
        .route(
            "/api/issues/issue-uuid/labels",
            post(move |Json(body): Json<Value>| async move {
                assert_eq!(body["label_id"], label_id);
                Json(serde_json::json!({
                    "labels":[{"id":label_id,"name":"Bug","color":"#ff0000"}]
                }))
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");
    let cli = Cli::try_parse_from([
        "patchbay", "issue", "label", "add", "CORD-18", "abcd", "--output", "json",
    ])
    .expect("issue label add CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("attach label");
    let labels: Value = serde_json::from_str(&output.stdout).expect("labels JSON");
    assert_eq!(labels[0]["name"], "Bug");
    task.abort();
}

#[tokio::test]
async fn issue_label_remove_preserves_success_when_refresh_fails() {
    let issue_id = "11111111-1111-1111-1111-111111111111";
    let label_id = "22222222-2222-2222-2222-222222222222";
    let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(move || async move {
                    Json(serde_json::json!({"id":issue_id,"identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/11111111-1111-1111-1111-111111111111/labels/22222222-2222-2222-2222-222222222222",
                delete_route(|| async { axum::http::StatusCode::NO_CONTENT }),
            )
            .route(
                "/api/issues/11111111-1111-1111-1111-111111111111/labels",
                get(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
            );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");
    let cli = Cli::try_parse_from([
        "patchbay", "issue", "label", "remove", "CORD-18", label_id, "--output", "json",
    ])
    .expect("issue label remove CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("detach label");
    assert_eq!(
        serde_json::from_str::<Value>(&output.stdout).expect("detach JSON"),
        serde_json::json!({"detached":true})
    );
    task.abort();
}
