use super::*;
use super::cli_test_helpers::*;
use axum::extract::Request;
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use url::form_urlencoded;

#[test]
fn issue_list_parser_matches_go_registry_flags() {
    let cli = Cli::try_parse_from([
        "cordy",
        "issue",
        "list",
        "--output",
        "json",
        "--full-id",
        "--status",
        "custom_status",
        "--priority",
        "urgent",
        "--assignee-id",
        "11111111-1111-1111-1111-111111111111",
        "--project",
        "abcd",
        "--metadata",
        "ready=true",
        "--metadata",
        "score=42",
        "--limit",
        "20",
        "--offset",
        "5",
        "--sort",
        "created_at",
        "--direction",
        "DESC",
    ])
    .expect("issue list CLI");
    let args = issue_list_args(&cli);
    assert_eq!(args.output, OutputFormat::Json);
    assert!(args.full_id);
    assert_eq!(args.status.as_deref(), Some("custom_status"));
    assert_eq!(args.priority.as_deref(), Some("urgent"));
    assert_eq!(args.project.as_deref(), Some("abcd"));
    assert_eq!(
        args.metadata,
        vec![String::from("ready=true"), String::from("score=42")]
    );
    assert_eq!((args.limit, args.offset), (20, 5));
    assert_eq!(args.sort.as_deref(), Some("created_at"));
    assert_eq!(args.direction.as_deref(), Some("DESC"));
}

#[test]
fn issue_list_metadata_filter_infers_primitives_and_rejects_duplicates() {
    let encoded = build_metadata_filter(&[
        "ready=true".into(),
        "score=42".into(),
        "forced=\"42\"".into(),
        "label=alpha".into(),
    ])
    .expect("metadata filter");
    let filter: Value = serde_json::from_str(&encoded).expect("metadata JSON");
    assert_eq!(filter["ready"], Value::Bool(true));
    assert_eq!(filter["score"], 42);
    assert_eq!(filter["forced"], "42");
    assert_eq!(filter["label"], "alpha");

    let error = build_metadata_filter(&["ready=true".into(), "ready=false".into()])
        .expect_err("duplicate metadata key");
    assert!(error.to_string().contains("given more than once"));
    let error =
        build_metadata_filter(&["missing-separator".into()]).expect_err("metadata key=value");
    assert!(error.to_string().contains("key=value form"));
}

#[test]
fn issue_list_has_more_uses_offset_and_returned_count() {
    assert!(issue_list_has_more(1, 1, 3));
    assert!(!issue_list_has_more(1, 2, 3));
    assert!(issue_list_has_more(0, 0, 1));
}

#[test]
fn issue_list_table_matches_go_columns_full_id_dates_and_actor_fallback() {
    let issues = vec![serde_json::json!({
        "id": "11111111-1111-1111-1111-111111111111",
        "identifier": "CORD-18",
        "title": "Migrate CLI",
        "status": "in_progress",
        "priority": "high",
        "assignee_type": "agent",
        "assignee_id": "22222222-2222-2222-2222-222222222222",
        "start_date": "2026-08-23T10:11:12Z",
        "due_date": "2026-08-30T00:00:00Z"
    })];
    let actors = IssueActorNames(HashMap::from([(
        "agent:22222222-2222-2222-2222-222222222222".into(),
        "CordyBot".into(),
    )]));
    let table = format_issue_list_table(&issues, true, &actors);
    assert!(table.starts_with("KEY"));
    assert!(table.contains("ID"));
    assert!(table.contains("CORD-18"));
    assert!(table.contains("11111111-1111-1111-1111-111111111111"));
    assert!(table.contains("agent:CordyBot"));
    assert!(table.contains("2026-08-23"));
    assert!(table.contains("2026-08-30"));

    let fallback = format_issue_list_table(&issues, false, &IssueActorNames::default());
    assert!(fallback.contains("agent:22222222-2222-2222-2222-222222222222"));
    assert!(!fallback.lines().next().unwrap_or_default().contains(" ID "));
}

#[tokio::test]
async fn issue_list_resolves_filters_and_sends_go_query_and_json_envelope() {
    let captured = Arc::new(Mutex::new(None::<String>));
    let captured_by_issues = Arc::clone(&captured);
    let app = Router::new()
        .route(
            "/api/workspaces/workspace-1/members",
            get(|| async {
                Json(serde_json::json!([{
                    "user_id": "11111111-1111-1111-1111-111111111111",
                    "name": "Ada Lovelace",
                    "email": "ada@example.com"
                }]))
            }),
        )
        .route("/api/agents", get(|| async { Json(serde_json::json!([])) }))
        .route("/api/squads", get(|| async { Json(serde_json::json!([])) }))
        .route(
            "/api/projects",
            get(|| async {
                Json(serde_json::json!({
                    "projects": [{
                        "id": "abcd0000-0000-0000-0000-000000000000",
                        "title": "Rust migration",
                        "status": "active"
                    }]
                }))
            }),
        )
        .route(
            "/api/issues",
            get(move |request: Request| {
                let captured = Arc::clone(&captured_by_issues);
                async move {
                    assert_eq!(request.headers()["authorization"], "Bearer token-1");
                    assert_eq!(request.headers()["x-workspace-id"], "workspace-1");
                    *captured.lock().expect("capture query") =
                        request.uri().query().map(Into::into);
                    Json(serde_json::json!({
                        "issues": [{
                            "id": "issue-1",
                            "identifier": "CORD-18",
                            "title": "Migrate CLI"
                        }],
                        "total": 3
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
    let cli = Cli::try_parse_from([
        "cordy",
        "issue",
        "list",
        "--output",
        "json",
        "--status",
        "custom_status",
        "--priority",
        "high",
        "--assignee",
        "Ada",
        "--project",
        "abcd",
        "--metadata",
        "ready=true",
        "--limit",
        "2",
        "--offset",
        "1",
        "--sort",
        "created_at",
        "--direction",
        "DESC",
    ])
    .expect("issue list CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("issue list");
    let envelope: Value = serde_json::from_str(&output.stdout).expect("list JSON");
    assert_eq!(envelope["total"], 3);
    assert_eq!(envelope["limit"], 2);
    assert_eq!(envelope["offset"], 1);
    assert_eq!(envelope["has_more"], Value::Bool(true));
    assert_eq!(envelope["issues"][0]["identifier"], "CORD-18");

    let query = captured
        .lock()
        .expect("captured query")
        .clone()
        .expect("query");
    let query = form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect::<HashMap<_, _>>();
    assert_eq!(query["workspace_id"], "workspace-1");
    assert_eq!(query["status"], "custom_status");
    assert_eq!(query["priority"], "high");
    assert_eq!(query["limit"], "2");
    assert_eq!(query["offset"], "1");
    assert_eq!(query["assignee_id"], "11111111-1111-1111-1111-111111111111");
    assert_eq!(query["project_id"], "abcd0000-0000-0000-0000-000000000000");
    assert_eq!(query["metadata"], r#"{"ready":true}"#);
    assert_eq!(query["sort"], "created_at");
    assert_eq!(query["direction"], "desc");
    task.abort();
}

#[tokio::test]
async fn issue_list_rejects_invalid_sort_direction_and_conflicting_assignee_flags() {
    let client = ApiClient::new(
        "http://127.0.0.1:1".into(),
        "workspace-1".into(),
        "token".into(),
        String::new(),
        String::new(),
        std::time::Duration::from_secs(1),
        CLIENT_VERSION,
    )
    .expect("client");
    for (argv, expected) in [
        (
            vec!["cordy", "issue", "list", "--sort", "nonsense"],
            "invalid --sort",
        ),
        (
            vec!["cordy", "issue", "list", "--direction", "desc"],
            "--direction requires --sort",
        ),
        (
            vec![
                "cordy",
                "issue",
                "list",
                "--sort",
                "created_at",
                "--direction",
                "sideways",
            ],
            "invalid --direction",
        ),
        (
            vec![
                "cordy",
                "issue",
                "list",
                "--sort",
                "position",
                "--direction",
                "asc",
            ],
            "--direction requires --sort",
        ),
        (
            vec![
                "cordy",
                "issue",
                "list",
                "--assignee",
                "Ada",
                "--assignee-id",
                "11111111-1111-1111-1111-111111111111",
            ],
            "mutually exclusive",
        ),
    ] {
        let cli = Cli::try_parse_from(argv).expect("CLI");
        let error = build_issue_list_query(&client, "workspace-1", issue_list_args(&cli))
            .await
            .expect_err("validation error");
        assert!(error.to_string().contains(expected), "{error:#}");
    }
}
