use super::cli_test_helpers::*;
use super::*;
use axum::extract::Request;
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use tokio::net::TcpListener;
#[test]
fn issue_run_controls_parser_and_message_table_match_go_contract() {
    let messages = Cli::try_parse_from([
        "cordy",
        "issue",
        "run-messages",
        "abcd",
        "--issue",
        "CORD-18",
        "--since",
        "4",
        "--output",
        "table",
    ])
    .expect("run-messages CLI");
    let args = issue_run_messages_args(&messages);
    assert_eq!(args.task_id, "abcd");
    assert_eq!(args.issue.as_deref(), Some("CORD-18"));
    assert_eq!(args.since, 4);
    assert_eq!(args.output, OutputFormat::Table);

    let cancel = Cli::try_parse_from([
        "cordy",
        "issue",
        "cancel-task",
        "11111111-1111-1111-1111-111111111111",
        "--output",
        "json",
    ])
    .expect("cancel-task CLI");
    assert_eq!(
        issue_cancel_task_args(&cancel).task_id,
        "11111111-1111-1111-1111-111111111111"
    );

    let table = format_issue_run_messages_table(&[
        serde_json::json!({
            "seq":1,"type":"text","tool":"","content":"done"
        }),
        serde_json::json!({
            "seq":2,"type":"tool_result","tool":"shell","content":"",
            "output":"x".repeat(81)
        }),
    ]);
    assert!(table.starts_with("SEQ"));
    assert!(table.contains("done"));
    assert!(table.contains("tool_result"));
    assert!(table.contains("xxx..."));
}

#[tokio::test]
async fn issue_run_messages_resolves_scoped_prefix_and_sends_since() {
    let issue_id = "1881a167-4bb6-4602-944b-f40ce4192fe6";
    let task_id = "abcd1234-0000-0000-0000-000000000000";
    let app =
        Router::new()
            .route(
                "/api/issues/CORD-18",
                get(move || async move {
                    Json(serde_json::json!({"id":issue_id,"identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/1881a167-4bb6-4602-944b-f40ce4192fe6/task-runs",
                get(move || async move { Json(vec![serde_json::json!({"id":task_id})]) }),
            )
            .route(
                "/api/tasks/abcd1234-0000-0000-0000-000000000000/messages",
                get(|request: Request| async move {
                    assert_eq!(request.uri().query(), Some("since=4"));
                    Json(vec![serde_json::json!({
                        "seq":5,"type":"text","content":"done"
                    })])
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
        "run-messages",
        "abcd",
        "--issue",
        "CORD-18",
        "--since",
        "4",
    ])
    .expect("run-messages CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("run messages");
    let messages: Value = serde_json::from_str(&output.stdout).expect("messages JSON");
    assert_eq!(messages[0]["seq"], 5);
    task.abort();
}

#[tokio::test]
async fn issue_cancel_task_posts_empty_body_and_requires_scope_for_prefix() {
    let task_id = "11111111-1111-1111-1111-111111111111";
    let app = Router::new().route(
        "/api/tasks/11111111-1111-1111-1111-111111111111/cancel",
        post(move |Json(body): Json<Value>| async move {
            assert_eq!(body, serde_json::json!({}));
            Json(serde_json::json!({"id":task_id,"status":"cancelled"}))
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
        "cancel-task",
        task_id,
        "--output",
        "table",
    ])
    .expect("cancel-task CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("cancel task");
    assert_eq!(
        output.stdout,
        "Task 11111111-1111-1111-1111-111111111111 -> status=cancelled\n"
    );

    let missing_scope =
        Cli::try_parse_from(["cordy", "issue", "cancel-task", "abcd"]).expect("short cancel CLI");
    let error = run_with_input(
        &missing_scope,
        &environment,
        &mut Cursor::new(Vec::<u8>::new()),
    )
    .await
    .expect_err("short task prefix requires issue");
    assert!(error.to_string().contains("require --issue"));
    task.abort();
}
