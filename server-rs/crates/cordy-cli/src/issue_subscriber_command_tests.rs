use super::*;
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use tokio::net::TcpListener;

#[test]
fn issue_subscriber_parser_and_table_match_go_contract() {
    let cli = Cli::try_parse_from([
        "cordy",
        "issue",
        "subscriber",
        "add",
        "CORD-18",
        "--user-id",
        "11111111-1111-1111-1111-111111111111",
        "--output",
        "table",
    ])
    .expect("subscriber add CLI");
    let Command::Issue(IssueArgs {
        command:
            IssueCommand::Subscriber(IssueSubscriberArgs {
                command: IssueSubscriberCommand::Add(args),
            }),
    }) = &cli.command
    else {
        panic!("expected subscriber add");
    };
    assert_eq!(args.issue_id, "CORD-18");
    assert_eq!(
        args.user_id.as_deref(),
        Some("11111111-1111-1111-1111-111111111111")
    );
    assert_eq!(args.output, OutputFormat::Table);

    let subscribers = [serde_json::json!({
        "user_type":"member","user_id":"member-1","reason":"manual",
        "created_at":"2026-08-24T12:34:56Z"
    })];
    let actors = IssueActorNames(HashMap::from([("member:member-1".into(), "Ada".into())]));
    let table = format_issue_subscribers_table(&subscribers, &actors);
    assert!(table.starts_with("USER"));
    assert!(table.contains("member:Ada"));
    assert!(table.contains("manual"));
    assert!(table.contains("2026-08-24T12:34"));
}

#[tokio::test]
async fn issue_subscriber_list_resolves_issue_and_preserves_json() {
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
        )
        .route(
            "/api/issues/issue-uuid/subscribers",
            get(|| async {
                Json(vec![serde_json::json!({
                    "user_type":"agent","user_id":"agent-1","reason":"mentioned",
                    "created_at":"2026-08-24T12:34:56Z"
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
        "subscriber",
        "list",
        "CORD-18",
        "--output",
        "json",
    ])
    .expect("subscriber list CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list subscribers");
    let subscribers: Value = serde_json::from_str(&output.stdout).expect("subscribers JSON");
    assert_eq!(subscribers[0]["user_id"], "agent-1");
    assert!(output.stderr.is_empty());
    task.abort();
}

#[tokio::test]
async fn issue_subscriber_mutation_defaults_to_caller_and_resolves_members_only() {
    let bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
    let subscribe_bodies = Arc::clone(&bodies);
    let unsubscribe_bodies = Arc::clone(&bodies);
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
        )
        .route(
            "/api/issues/issue-uuid/subscribe",
            post(move |Json(body): Json<Value>| {
                let bodies = Arc::clone(&subscribe_bodies);
                async move {
                    bodies.lock().expect("bodies").push(body);
                    Json(serde_json::json!({"subscribed":true}))
                }
            }),
        )
        .route(
            "/api/issues/issue-uuid/unsubscribe",
            post(move |Json(body): Json<Value>| {
                let bodies = Arc::clone(&unsubscribe_bodies);
                async move {
                    bodies.lock().expect("bodies").push(body);
                    Json(serde_json::json!({"subscribed":false}))
                }
            }),
        )
        .route(
            "/api/workspaces/workspace-1/members",
            get(|| async {
                Json(vec![serde_json::json!({
                    "user_id":"11111111-1111-1111-1111-111111111111","name":"Ada",
                    "email":"ada@example.com"
                })])
            }),
        )
        .route("/api/agents", get(|| async { Json(Vec::<Value>::new()) }));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");
    environment.set("CORDY_TOKEN", "token-1");

    let caller = Cli::try_parse_from(["cordy", "issue", "subscriber", "add", "CORD-18"])
        .expect("subscriber caller CLI");
    let caller_output = run_with_input(&caller, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("subscribe caller");
    assert_eq!(
        caller_output.stderr,
        "Subscribed caller to issue CORD-18.\n"
    );

    let member = Cli::try_parse_from([
        "cordy",
        "issue",
        "subscriber",
        "remove",
        "CORD-18",
        "--user-id",
        "11111111-1111-1111-1111-111111111111",
        "--output",
        "table",
    ])
    .expect("subscriber member CLI");
    let member_output = run_with_input(&member, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("unsubscribe member");
    assert!(member_output.stdout.is_empty());
    assert_eq!(
        member_output.stderr,
        "Unsubscribed member:Ada to issue CORD-18.\n"
    );
    assert_eq!(
        *bodies.lock().expect("bodies"),
        vec![
            serde_json::json!({}),
            serde_json::json!({
                "user_type":"member",
                "user_id":"11111111-1111-1111-1111-111111111111"
            })
        ]
    );
    task.abort();
}
