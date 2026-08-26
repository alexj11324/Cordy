use super::*;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

#[test]
fn login_callback_state_comparison_is_exact_and_handles_length_mismatch() {
    assert!(constant_time_equal(b"state", b"state"));
    assert!(!constant_time_equal(b"state", b"attacker"));
    assert!(!constant_time_equal(b"state", b"state\0"));
}

#[test]
fn login_url_encodes_callback_and_state_without_leaking_raw_query_delimiters() {
    let url = build_login_url(
        "https://cordy.example/base",
        "http://127.0.0.1:1234/callback?reserved=yes",
        "state+with spaces",
    )
    .expect("login URL");
    let parsed = Url::parse(&url).expect("parsed login URL");
    assert_eq!(parsed.path(), "/base/login");
    assert_eq!(
        parsed
            .query_pairs()
            .find(|(key, _)| key == "cli_state")
            .unwrap()
            .1,
        "state+with spaces"
    );
    assert_eq!(
        parsed
            .query_pairs()
            .find(|(key, _)| key == "cli_callback")
            .unwrap()
            .1,
        "http://127.0.0.1:1234/callback?reserved=yes"
    );
    assert!(!url.contains("cli_callback=http://127.0.0.1:1234/callback?reserved"));
}

#[test]
fn authenticated_profile_save_resets_stale_workspace_atomically() {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment
        .set_profile_value("", "workspace_id", Some(Value::String("old".into())))
        .expect("seed config");
    environment
        .save_authenticated_profile(
            "",
            "https://api.new.example",
            "https://app.new.example",
            "mul_new_secret",
            "new-workspace",
        )
        .expect("save authenticated profile");
    let config = environment.load_config("").expect("load config");
    assert_eq!(config.server_url, "https://api.new.example");
    assert_eq!(config.app_url, "https://app.new.example");
    assert_eq!(config.workspace_id, "new-workspace");
    assert_eq!(config.token, "mul_new_secret");
}

#[tokio::test]
async fn login_callback_rejects_wrong_state_then_accepts_matching_state() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let address = listener.local_addr().expect("address");
    let waiter = tokio::spawn(wait_for_login_callback(listener, "expected".into()));

    let mut attacker = tokio::net::TcpStream::connect(address)
        .await
        .expect("attacker connection");
    attacker
        .write_all(
            b"GET /callback?state=wrong&token=mul_secret HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await
        .expect("attacker request");
    let mut response = Vec::new();
    attacker
        .read_to_end(&mut response)
        .await
        .expect("attacker response");
    assert!(String::from_utf8_lossy(&response).contains("400 Bad Request"));

    let mut browser = tokio::net::TcpStream::connect(address)
        .await
        .expect("browser connection");
    browser
        .write_all(
            b"GET /callback?state=expected&token=mul_secret HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await
        .expect("browser request");
    let mut response = Vec::new();
    browser
        .read_to_end(&mut response)
        .await
        .expect("browser response");
    assert!(String::from_utf8_lossy(&response).contains("200 OK"));
    assert_eq!(
        waiter.await.expect("callback task").expect("token"),
        "mul_secret"
    );
}

#[test]
fn workspace_creation_url_is_safe_and_discards_untrusted_query() {
    let url =
        build_workspace_creation_url("https://app.example/base?next=https://evil.example#fragment")
            .expect("workspace URL");
    assert_eq!(url, "https://app.example/base/workspaces/new");
    assert!(build_workspace_creation_url("javascript:alert(1)").is_err());
}

#[tokio::test]
async fn workspace_creation_polling_opens_url_and_returns_new_workspace() {
    let requests = Arc::new(Mutex::new(0_u32));
    let route_requests = Arc::clone(&requests);
    let app = Router::new().route(
        "/api/workspaces",
        get(move || {
            let requests = Arc::clone(&route_requests);
            async move {
                let mut count = requests.lock().expect("request count");
                *count += 1;
                if *count == 1 {
                    Json(serde_json::json!([]))
                } else {
                    Json(serde_json::json!([{"id":"workspace-new","name":"New workspace"}]))
                }
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    let client = ApiClient::new(
        format!("http://{address}"),
        String::new(),
        "mul_test".into(),
        String::new(),
        String::new(),
        Duration::from_secs(1),
        CLIENT_VERSION,
    )
    .expect("client");
    let opened = Arc::new(Mutex::new(String::new()));
    let opened_for_test = Arc::clone(&opened);
    let workspaces = wait_for_workspace_creation_with_opener(
        &client,
        "https://app.example/base",
        Duration::from_millis(1),
        Duration::from_secs(1),
        move |url| *opened_for_test.lock().expect("opened URL") = url.to_owned(),
    )
    .await
    .expect("workspace discovery");
    assert_eq!(workspaces[0].id, "workspace-new");
    assert_eq!(
        opened.lock().expect("opened URL").as_str(),
        "https://app.example/base/workspaces/new"
    );
    server.abort();
}
