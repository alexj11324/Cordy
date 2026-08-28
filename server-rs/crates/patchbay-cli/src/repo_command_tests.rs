use super::*;
use axum::extract::Request;
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[tokio::test]
async fn repo_registry_add_remove_and_list_match_go_patch_contracts() {
    let repos = Arc::new(Mutex::new(vec![WorkspaceRepo {
        url: "https://git.example.com/web.git".into(),
        description: "web".into(),
    }]));
    let repos_get = Arc::clone(&repos);
    let repos_patch = Arc::clone(&repos);
    let app = Router::new().route(
        "/api/workspaces/ws-1",
        get(move || {
            let repos = Arc::clone(&repos_get);
            async move {
                Json(serde_json::json!({
                    "id":"ws-1","repos":repos.lock().expect("repos").clone()
                }))
            }
        })
        .patch(move |Json(body): Json<Value>| {
            let repos = Arc::clone(&repos_patch);
            async move {
                let updated: Vec<WorkspaceRepo> =
                    serde_json::from_value(body["repos"].clone()).expect("repo patch body");
                *repos.lock().expect("repos") = updated.clone();
                Json(serde_json::json!({"id":"ws-1","repos":updated}))
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_WORKSPACE_ID", "ws-1");
    environment.set("PATCHBAY_TOKEN", "token-1");

    let add = Cli::try_parse_from([
        "patchbay",
        "repo",
        "add",
        "https://git.example.com/api.git",
        "https://git.example.com/api.git",
        "--url",
        "https://git.example.com/web.git",
        "--output",
        "json",
    ])
    .expect("repo add CLI");
    let added = run_with_input(&add, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("add repos");
    let added: Value = serde_json::from_str(&added.stdout).expect("add JSON");
    assert_eq!(added["added"].as_array().expect("added").len(), 1);
    assert_eq!(added["repos"].as_array().expect("repos").len(), 2);

    let remove = Cli::try_parse_from([
        "patchbay",
        "repo",
        "rm",
        "https://git.example.com/web.git",
        "--output",
        "table",
    ])
    .expect("repo remove alias");
    let removed = run_with_input(&remove, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("remove repo");
    assert!(removed.stdout.starts_with("REMOVED URL"));
    assert!(removed.stdout.contains("web.git"));

    let list = Cli::try_parse_from(["patchbay", "repo", "list", "--output", "table"])
        .expect("repo list CLI");
    let listed = run_with_input(&list, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list repos");
    assert!(listed.stdout.starts_with("URL"));
    assert!(listed.stdout.contains("api.git"));
    assert!(!listed.stdout.contains("web.git"));
    server.abort();
}

#[test]
fn repo_registry_rejects_empty_duplicate_and_invalid_description_inputs() {
    assert_eq!(
        repo_urls(&[" a ".into()], &["a".into(), "b".into()]).expect("dedupe"),
        vec!["a", "b"]
    );
    assert!(repo_urls(&[], &[])
        .expect_err("missing URL")
        .to_string()
        .contains("at least one"));
    assert!(repo_urls(&[" ".into()], &[])
        .expect_err("empty URL")
        .to_string()
        .contains("cannot be empty"));
    assert!(Cli::try_parse_from([
        "patchbay",
        "repo",
        "remove",
        "https://git.example.com/a.git",
        "--description",
        "x"
    ])
    .is_err());
}

#[tokio::test]
async fn repo_checkout_forwards_task_context_and_retries_only_marked_busy() {
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let attempts_handler = Arc::clone(&attempts);
    let app = Router::new().route(
        "/repo/checkout",
        post(move |request: Request| {
            let attempts = Arc::clone(&attempts_handler);
            async move {
                let attempt = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                assert_eq!(
                    request
                        .headers()
                        .get("authorization")
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer mat_checkout")
                );
                let body = axum::body::to_bytes(request.into_body(), usize::MAX)
                    .await
                    .expect("checkout body");
                let body: Value = serde_json::from_slice(&body).expect("checkout JSON");
                assert_eq!(body["url"], "https://github.com/acme/patchbay.git");
                assert_eq!(body["workspace_id"], "ws-1");
                assert_eq!(body["agent_name"], "Rust Agent");
                assert_eq!(body["task_id"], "task-1");
                assert_eq!(body["checkout_mode"], "isolated");
                assert_eq!(body["ref"], "release/v2");
                assert_eq!(body["retry_busy"], true);
                if attempt == 0 {
                    let mut response = axum::response::Response::builder()
                        .status(axum::http::StatusCode::SERVICE_UNAVAILABLE)
                        .header("X-Patchbay-Retryable", "repo-busy")
                        .header("Retry-After", "0")
                        .body(axum::body::Body::from("busy"))
                        .expect("busy response");
                    response
                        .headers_mut()
                        .insert("content-type", "text/plain".parse().expect("content type"));
                    return response;
                }
                axum::response::Response::builder()
                    .status(axum::http::StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"path":"/work/patchbay","branch_name":"agent/rust/task-1"}"#,
                    ))
                    .expect("success response")
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("address").port();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_DAEMON_PORT", port.to_string());
    environment.set("PATCHBAY_WORKSPACE_ID", "ws-1");
    environment.set("PATCHBAY_AGENT_NAME", "Rust Agent");
    environment.set("PATCHBAY_TASK_ID", "task-1");
    environment.set("PATCHBAY_TOKEN", "mat_checkout");
    environment.set("PATCHBAY_REPO_CHECKOUT_MODE", " isolated ");
    let cli = Cli::try_parse_from([
        "patchbay",
        "repo",
        "checkout",
        "https://github.com/acme/patchbay.git",
        "--ref",
        "release/v2",
    ])
    .expect("repo checkout CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("repo checkout");
    assert_eq!(output.stdout, "/work/patchbay\n");
    assert!(output.stderr.contains("branch: agent/rust/task-1"));
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
    server.abort();
}

#[test]
fn repo_checkout_retry_delay_matches_go_seconds_date_and_caps() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-08-24T00:00:00Z")
        .expect("now")
        .with_timezone(&chrono::Utc);
    assert_eq!(
        repo_checkout_retry_delay("7", now),
        std::time::Duration::from_secs(7)
    );
    assert_eq!(
        repo_checkout_retry_delay("60", now),
        std::time::Duration::from_secs(30)
    );
    assert_eq!(
        repo_checkout_retry_delay("Mon, 24 Aug 2026 00:00:05 GMT", now),
        std::time::Duration::from_secs(5)
    );
    assert_eq!(
        repo_checkout_retry_delay("invalid", now),
        std::time::Duration::from_secs(1)
    );
}
