use super::*;
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
#[test]
fn issue_pull_request_attach_parser_requires_url_and_matches_go_flags() {
    assert!(Cli::try_parse_from(["cordy", "issue", "pull-request", "attach", "CORD-18"]).is_err());
    let cli = Cli::try_parse_from([
        "cordy",
        "issue",
        "pull-request",
        "attach",
        "CORD-18",
        "--url",
        "https://github.com/owner/repo/pull/42",
        "--title",
        "Rust CLI",
        "--state",
        "open",
        "--branch",
        "cli",
        "--head-sha",
        "abc123",
        "--output",
        "json",
    ])
    .expect("attach CLI");
    match cli.command {
        Command::Issue(IssueArgs {
            command:
                IssueCommand::PullRequest(IssuePullRequestArgs {
                    command: IssuePullRequestCommand::Attach(args),
                }),
        }) => {
            assert_eq!(args.issue_id, "CORD-18");
            assert_eq!(args.url, "https://github.com/owner/repo/pull/42");
            assert_eq!(args.title.as_deref(), Some("Rust CLI"));
            assert_eq!(args.state.as_deref(), Some("open"));
            assert_eq!(args.branch.as_deref(), Some("cli"));
            assert_eq!(args.head_sha.as_deref(), Some("abc123"));
            assert_eq!(args.output, OutputFormat::Json);
        }
        _ => panic!("expected issue pull-request attach"),
    }
}

#[tokio::test]
async fn issue_pull_request_attach_rejects_empty_url_with_go_guidance() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let cli = Cli::try_parse_from([
        "cordy",
        "issue",
        "pull-request",
        "attach",
        "CORD-18",
        "--url",
        "",
    ])
    .expect("empty URL reaches runtime validation");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("empty URL");
    assert_eq!(
        error.to_string(),
        "--url is required (https://github.com/{owner}/{repo}/pull/{number})"
    );
}

#[tokio::test]
async fn issue_pull_request_attach_posts_trimmed_url_and_optional_metadata() {
    let captured = Arc::new(Mutex::new(None::<Value>));
    let captured_by_handler = Arc::clone(&captured);
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(|| async {
                Json(serde_json::json!({
                    "id": "11111111-1111-1111-1111-111111111111",
                    "identifier": "CORD-18"
                }))
            }),
        )
        .route(
            "/api/issues/11111111-1111-1111-1111-111111111111/pull-requests",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let captured = Arc::clone(&captured_by_handler);
                async move {
                    assert_eq!(headers["authorization"], "Bearer token-1");
                    *captured.lock().expect("capture body") = Some(body);
                    Json(serde_json::json!({
                        "pull_request": {
                            "number": 42,
                            "state": "open",
                            "title": "Rust CLI",
                            "url": "https://github.com/owner/repo/pull/42"
                        }
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
        "pull-request",
        "attach",
        "CORD-18",
        "--url",
        "  https://github.com/owner/repo/pull/42  ",
        "--title",
        "Rust CLI",
        "--state",
        "   ",
        "--branch",
        "cli",
        "--output",
        "json",
    ])
    .expect("attach CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("attach pull request");
    let result: Value = serde_json::from_str(&output.stdout).expect("attach JSON");
    assert_eq!(result["pull_request"]["number"], 42);
    let body = captured
        .lock()
        .expect("captured body")
        .clone()
        .expect("body");
    assert_eq!(body["url"], "https://github.com/owner/repo/pull/42");
    assert_eq!(body["title"], "Rust CLI");
    assert_eq!(body["branch"], "cli");
    assert!(body.get("state").is_none());
    assert!(body.get("head_sha").is_none());
    task.abort();
}
