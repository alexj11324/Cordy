use super::*;
use axum::extract::Request;
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[test]
fn issue_get_parser_defaults_to_json_and_accepts_only_one_reference() {
    let cli = Cli::try_parse_from(["cordy", "issue", "get", "CORD-18"]).expect("issue get CLI");
    match cli.command {
        Command::Issue(IssueArgs {
            command: IssueCommand::Get { id, output },
        }) => {
            assert_eq!(id, "CORD-18");
            assert_eq!(output, OutputFormat::Json);
        }
        _ => panic!("expected issue get"),
    }
    assert!(Cli::try_parse_from(["cordy", "issue", "get"]).is_err());
    assert!(Cli::try_parse_from(["cordy", "issue", "get", "A-1", "B-2"]).is_err());
    assert!(Cli::try_parse_from(["cordy", "issue", "get", "CORD-18", "--output", "table"]).is_ok());
}

#[tokio::test]
async fn issue_ref_rejects_short_uuid_and_invalid_inputs_without_http() {
    let client = ApiClient::new(
        "http://127.0.0.1:1".into(),
        "workspace-1".into(),
        "token".into(),
        String::new(),
        String::new(),
        std::time::Duration::from_millis(50),
        CLIENT_VERSION,
    )
    .expect("client");
    for input in ["1881", "1881-a167", "1852"] {
        let error = resolve_issue_ref(&client, input)
            .await
            .expect_err("short prefix");
        assert!(error.to_string().contains("short UUID prefix"));
        assert!(error.to_string().contains("MUL-123"));
    }
    let error = resolve_issue_ref(&client, "not-an-id")
        .await
        .expect_err("invalid ref");
    assert!(error
        .to_string()
        .contains("not a recognized issue reference"));
    assert!(!error.to_string().contains("short UUID prefix"));
}

#[tokio::test]
async fn issue_get_resolves_key_then_fetches_canonical_issue() {
    let hits = Arc::new(Mutex::new(Vec::<String>::new()));
    let first_hits = Arc::clone(&hits);
    let second_hits = Arc::clone(&hits);
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(move || {
                let hits = Arc::clone(&first_hits);
                async move {
                    hits.lock().expect("hits").push("CORD-18".into());
                    Json(serde_json::json!({
                        "id": "11111111-1111-1111-1111-111111111111",
                        "identifier": "CORD-18",
                        "title": "Resolver response"
                    }))
                }
            }),
        )
        .route(
            "/api/issues/11111111-1111-1111-1111-111111111111",
            get(move |request: Request| {
                let hits = Arc::clone(&second_hits);
                async move {
                    assert_eq!(request.headers()["authorization"], "Bearer token-1");
                    assert_eq!(request.headers()["x-workspace-id"], "workspace-1");
                    hits.lock().expect("hits").push("canonical".into());
                    Json(serde_json::json!({
                        "id": "11111111-1111-1111-1111-111111111111",
                        "identifier": "CORD-18",
                        "title": "Canonical issue",
                        "description": "Full details"
                    }))
                }
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");
    environment.set("CORDY_TOKEN", "token-1");
    let cli = Cli::try_parse_from(["cordy", "issue", "get", "CORD-18"]).expect("issue get CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("issue get");
    let issue: Value = serde_json::from_str(&output.stdout).expect("issue JSON");
    assert_eq!(issue["title"], "Canonical issue");
    assert_eq!(issue["description"], "Full details");
    assert_eq!(
        *hits.lock().expect("hits"),
        vec![String::from("CORD-18"), String::from("canonical")]
    );
    task.abort();
}

#[test]
fn issue_get_table_matches_go_detail_columns() {
    let issue = serde_json::json!({
        "id": "11111111-1111-1111-1111-111111111111",
        "identifier": "CORD-18",
        "title": "Migrate get",
        "status": "in_progress",
        "priority": "high",
        "assignee_type": "member",
        "assignee_id": "22222222-2222-2222-2222-222222222222",
        "start_date": "2026-08-24T10:00:00Z",
        "due_date": "2026-08-31T10:00:00Z",
        "description": "Preserve the complete description"
    });
    let actors = IssueActorNames(HashMap::from([(
        "member:22222222-2222-2222-2222-222222222222".into(),
        "Ada".into(),
    )]));
    let table = format_issue_get_table(&issue, &actors);
    assert!(table.starts_with("KEY"));
    assert!(table.contains("DESCRIPTION"));
    assert!(table.contains("CORD-18"));
    assert!(table.contains("member:Ada"));
    assert!(table.contains("2026-08-24"));
    assert!(table.contains("2026-08-31"));
    assert!(table.contains("Preserve the complete description"));
}
