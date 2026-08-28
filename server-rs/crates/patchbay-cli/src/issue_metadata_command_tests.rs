use super::*;
use axum::routing::{get, put};
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use tokio::net::TcpListener;

#[test]
fn issue_metadata_parser_value_types_and_table_match_go_contract() {
    let cli = Cli::try_parse_from([
        "patchbay", "issue", "metadata", "set", "CORD-18", "--key", "attempt", "--value=", "--type",
        "string", "--output", "json",
    ])
    .expect("metadata set CLI");
    let Command::Issue(IssueArgs {
        command:
            IssueCommand::Metadata(IssueMetadataArgs {
                command: IssueMetadataCommand::Set(args),
            }),
    }) = &cli.command
    else {
        panic!("expected metadata set");
    };
    assert_eq!(args.key.as_deref(), Some("attempt"));
    assert_eq!(args.value.as_deref(), Some(""));
    assert_eq!(args.value_type.as_deref(), Some("string"));
    assert_eq!(
        parse_metadata_value("true", None).expect("bool"),
        Value::Bool(true)
    );
    assert_eq!(
        parse_metadata_value("3.5", None).expect("number"),
        serde_json::json!(3.5)
    );
    assert_eq!(
        parse_metadata_value("42", Some("string")).expect("forced string"),
        Value::String("42".into())
    );
    assert!(parse_metadata_value("yes", Some("bool"))
        .expect_err("invalid bool")
        .to_string()
        .contains("expected true or false"));

    let metadata = serde_json::Map::from_iter([
        ("zeta".into(), serde_json::json!(2)),
        ("alpha".into(), serde_json::json!(true)),
    ]);
    let table = format_metadata_table(&metadata);
    assert!(table.starts_with("KEY"));
    assert!(table.find("alpha").expect("alpha") < table.find("zeta").expect("zeta"));
    assert!(table.contains("bool"));
    assert!(table.contains("number"));
}

#[tokio::test]
async fn issue_metadata_list_degrades_only_not_found_to_empty() {
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
        )
        .route(
            "/api/issues/issue-uuid/metadata",
            get(|| async { axum::http::StatusCode::NOT_FOUND }),
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
        "patchbay", "issue", "metadata", "list", "CORD-18", "--output", "json",
    ])
    .expect("metadata list CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("metadata list fallback");
    assert_eq!(
        serde_json::from_str::<Value>(&output.stdout).expect("metadata JSON"),
        serde_json::json!({})
    );
    task.abort();
}

#[tokio::test]
async fn issue_metadata_set_puts_typed_value_and_returns_full_map() {
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
        )
        .route(
            "/api/issues/issue-uuid/metadata/attempt",
            put(|Json(body): Json<Value>| async move {
                assert_eq!(body, serde_json::json!({"value":3}));
                Json(serde_json::json!({"metadata":{"attempt":3,"ready":true}}))
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
        "patchbay", "issue", "metadata", "set", "CORD-18", "--key", "attempt", "--value", "3",
        "--type", "number", "--output", "json",
    ])
    .expect("metadata set CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("set metadata");
    let metadata: Value = serde_json::from_str(&output.stdout).expect("metadata JSON");
    assert_eq!(metadata["attempt"], 3);
    assert_eq!(metadata["ready"], true);
    task.abort();
}
