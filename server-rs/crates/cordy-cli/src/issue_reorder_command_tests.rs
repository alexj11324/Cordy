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
#[test]
fn issue_reorder_parser_enforces_exactly_one_real_target() {
    assert!(Cli::try_parse_from(["cordy", "issue", "reorder", "CORD-18"]).is_err());
    assert!(
        Cli::try_parse_from(["cordy", "issue", "reorder", "CORD-18", "--top", "--bottom"]).is_err()
    );
    let cli = Cli::try_parse_from([
        "cordy", "issue", "reorder", "CORD-18", "--before", "CORD-1", "--output", "table",
    ])
    .expect("reorder CLI");
    let args = issue_reorder_args(&cli);
    assert_eq!(args.id, "CORD-18");
    assert_eq!(args.before.as_deref(), Some("CORD-1"));
    assert_eq!(args.output, OutputFormat::Table);

    let false_top = Cli::try_parse_from(["cordy", "issue", "reorder", "CORD-18", "--top=false"])
        .expect("false bool reaches runtime");
    assert_eq!(issue_reorder_args(&false_top).top, Some(false));
}

#[test]
fn issue_reorder_position_math_matches_board_drag_contract() {
    let positions = HashMap::from([
        (String::from("one"), 10.0),
        (String::from("two"), 20.0),
        (String::from("three"), 40.0),
    ]);
    assert_eq!(
        compute_reorder_position(
            &["two".into(), "one".into(), "three".into()],
            "two",
            &positions,
            20.0,
        ),
        9.0
    );
    assert_eq!(
        compute_reorder_position(
            &["one".into(), "two".into(), "three".into()],
            "two",
            &positions,
            20.0,
        ),
        25.0
    );
    assert_eq!(
        compute_reorder_position(
            &["one".into(), "three".into(), "two".into()],
            "two",
            &positions,
            20.0,
        ),
        41.0
    );
}

#[tokio::test]
async fn issue_reorder_paginates_project_column_and_puts_computed_position() {
    let captured = Arc::new(Mutex::new(None::<Value>));
    let captured_by_update = Arc::clone(&captured);
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(|| async { Json(serde_json::json!({"id":"target-id","identifier":"CORD-18"})) }),
        )
        .route(
            "/api/issues/CORD-1",
            get(|| async { Json(serde_json::json!({"id":"other-id","identifier":"CORD-1"})) }),
        )
        .route(
            "/api/issues/target-id",
            get(|| async {
                Json(serde_json::json!({
                    "id":"target-id","identifier":"CORD-18","title":"Target",
                    "status":"todo","priority":"high","project_id":"project-1","position":20.0
                }))
            })
            .put(move |Json(body): Json<Value>| {
                let captured = Arc::clone(&captured_by_update);
                async move {
                    *captured.lock().expect("capture reorder") = Some(body.clone());
                    Json(serde_json::json!({
                        "id":"target-id","identifier":"CORD-18","title":"Target",
                        "status":"todo","priority":"high","position":body["position"]
                    }))
                }
            }),
        )
        .route(
            "/api/issues",
            get(|request: Request| async move {
                let query = request.uri().query().unwrap_or_default();
                assert!(query.contains("workspace_id=workspace-1"));
                assert!(query.contains("status=todo"));
                assert!(query.contains("project_id=project-1"));
                assert!(query.contains("sort=position"));
                if query.contains("offset=0") {
                    Json(serde_json::json!({
                        "issues":[
                            {"id":"other-id","position":10.0},
                            {"id":"target-id","position":20.0}
                        ],
                        "total":3
                    }))
                } else {
                    assert!(query.contains("offset=2"));
                    Json(serde_json::json!({
                        "issues":[{"id":"last-id","position":30.0}],
                        "total":3
                    }))
                }
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
        "cordy", "issue", "reorder", "CORD-18", "--before", "CORD-1", "--output", "table",
    ])
    .expect("reorder CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("reorder issue");
    assert_eq!(output.stderr, "Issue CORD-18 reordered.\n");
    assert!(output.stdout.starts_with("KEY"));
    assert_eq!(
        captured
            .lock()
            .expect("body")
            .clone()
            .expect("captured body")["position"],
        9.0
    );
    task.abort();
}

#[tokio::test]
async fn issue_reorder_rejects_false_selector_before_network() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let cli = Cli::try_parse_from(["cordy", "issue", "reorder", "CORD-18", "--bottom=false"])
        .expect("false bool reaches runtime");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("false selector");
    assert!(error.to_string().contains("cannot be set to false"));
}
