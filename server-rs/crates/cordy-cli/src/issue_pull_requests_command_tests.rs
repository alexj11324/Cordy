use super::*;
use axum::extract::Request;
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
#[test]
fn issue_pull_requests_parser_supports_go_name_alias_and_defaults() {
    for name in ["pull-requests", "prs"] {
        let cli =
            Cli::try_parse_from(["cordy", "issue", name, "CORD-18"]).expect("pull requests CLI");
        match cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::PullRequests { id, output },
            }) => {
                assert_eq!(id, "CORD-18");
                assert_eq!(output, OutputFormat::Table);
            }
            _ => panic!("expected issue pull-requests"),
        }
    }
    assert!(Cli::try_parse_from([
        "cordy",
        "issue",
        "pull-requests",
        "CORD-18",
        "--output",
        "json"
    ])
    .is_ok());
}

#[tokio::test]
async fn issue_pull_requests_resolves_issue_and_preserves_json_wrapper() {
    let hits = Arc::new(Mutex::new(Vec::<String>::new()));
    let resolve_hits = Arc::clone(&hits);
    let pull_request_hits = Arc::clone(&hits);
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(move || {
                let hits = Arc::clone(&resolve_hits);
                async move {
                    hits.lock().expect("hits").push("resolve".into());
                    Json(serde_json::json!({
                        "id": "11111111-1111-1111-1111-111111111111",
                        "identifier": "CORD-18"
                    }))
                }
            }),
        )
        .route(
            "/api/issues/11111111-1111-1111-1111-111111111111/pull-requests",
            get(move |request: Request| {
                let hits = Arc::clone(&pull_request_hits);
                async move {
                    assert_eq!(request.headers()["authorization"], "Bearer token-1");
                    assert_eq!(request.headers()["x-workspace-id"], "workspace-1");
                    hits.lock().expect("hits").push("pull-requests".into());
                    Json(serde_json::json!({
                        "pull_requests": [{
                            "number": 42,
                            "state": "open",
                            "title": "Rust CLI",
                            "url": "https://github.example/pr/42"
                        }],
                        "count": 1
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
    let cli = Cli::try_parse_from(["cordy", "issue", "prs", "CORD-18", "--output", "json"])
        .expect("pull requests CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("pull requests");
    let result: Value = serde_json::from_str(&output.stdout).expect("pull request JSON");
    assert_eq!(result["count"], 1);
    assert_eq!(result["pull_requests"][0]["number"], 42);
    assert_eq!(
        *hits.lock().expect("hits"),
        vec![String::from("resolve"), String::from("pull-requests")]
    );
    task.abort();
}

#[test]
fn issue_pull_requests_table_uses_url_then_html_url_fallback() {
    let result = serde_json::json!({
        "pull_requests": [
            {
                "number": 42,
                "state": "open",
                "title": "Direct URL",
                "url": "https://github.example/pr/42",
                "html_url": "https://ignored.example/pr/42"
            },
            {
                "number": 43,
                "state": "merged",
                "title": "Fallback URL",
                "html_url": "https://github.example/pr/43"
            }
        ]
    });
    let table = format_issue_pull_requests_table(&result);
    assert!(table.starts_with("NUMBER"));
    assert!(table.contains("Direct URL"));
    assert!(table.contains("https://github.example/pr/42"));
    assert!(!table.contains("https://ignored.example/pr/42"));
    assert!(table.contains("Fallback URL"));
    assert!(table.contains("https://github.example/pr/43"));
}
