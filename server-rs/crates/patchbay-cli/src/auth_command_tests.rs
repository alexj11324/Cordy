use super::*;
use axum::extract::Request;
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use std::fs;
use std::io::Cursor;
use tokio::net::TcpListener;

#[tokio::test]
async fn auth_status_matches_human_table_and_json_contracts() {
    let app = Router::new().route(
        "/api/me",
        get(|request: Request| async move {
            assert_eq!(
                request.headers()["authorization"],
                "Bearer pby_env_status_token"
            );
            assert!(request.headers().get("x-workspace-id").is_none());
            assert!(request.headers().get("x-agent-id").is_none());
            Json(serde_json::json!({"name":"Ada","email":"ada@example.com"}))
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_TOKEN", "pby_env_status_token");

    let table = Cli::try_parse_from(["patchbay", "auth", "status"]).expect("status CLI");
    let output = run_with_input(&table, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("table status");
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        format!(
            "Server:  http://{address}\nUser:    Ada (ada@example.com)\nToken:   {}\n",
            display_token_prefix("pby_env_status_token")
        )
    );

    let json = Cli::try_parse_from(["patchbay", "auth", "status", "--output", "json"])
        .expect("JSON status CLI");
    let output = run_with_input(&json, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("JSON status");
    let status: Value = serde_json::from_str(&output.stdout).expect("status JSON");
    assert_eq!(status["authenticated"], true);
    assert_eq!(status["user"]["email"], "ada@example.com");
    assert_eq!(
        status["token"],
        display_token_prefix("pby_env_status_token")
    );
    server.abort();
}

#[tokio::test]
async fn auth_status_task_context_requires_mat_token_and_never_prints_it() {
    let app = Router::new().route(
        "/api/me",
        get(|request: Request| async move {
            assert_eq!(
                request.headers()["authorization"],
                "Bearer mat_task_status_secret"
            );
            Json(serde_json::json!({"name":"Task Agent","email":"task@example.test"}))
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let task_root = tempfile::tempdir().expect("task root");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_AGENT_ID", "agent-1");
    environment.set("PATCHBAY_TASK_ID", "task-1");
    environment.set("PATCHBAY_TOKEN", "mat_task_status_secret");
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    let cli = Cli::try_parse_from(["patchbay", "auth", "status", "--output", "json"])
        .expect("task status CLI");
    let missing_root = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("task-local config root required");
    assert!(missing_root
        .to_string()
        .contains(config::TASK_CONFIG_ROOT_ENV));

    environment.set(
        config::TASK_CONFIG_ROOT_ENV,
        task_root.path().display().to_string(),
    );
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("task status");
    assert!(!output.stdout.contains("mat_task_status_secret"));
    assert!(serde_json::from_str::<Value>(&output.stdout)
        .expect("task status JSON")
        .get("token")
        .is_none());

    environment.set("PATCHBAY_TOKEN", "pby_owner_token");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("human token rejected in task");
    assert!(error.to_string().contains("task-scoped mat_ token"));
    server.abort();
}

#[test]
fn auth_logout_only_clears_current_profile_and_is_task_guarded() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let default_path = home.path().join(".patchbay/config.json");
    let profile_path = home.path().join(".patchbay/profiles/dev/config.json");
    fs::create_dir_all(default_path.parent().expect("default parent")).expect("default dir");
    fs::create_dir_all(profile_path.parent().expect("profile parent")).expect("profile dir");
    let default_bytes = br#"{"token":"pby_default","workspace_id":"default"}"#;
    fs::write(&default_path, default_bytes).expect("default config");
    fs::write(
        &profile_path,
        r#"{"token":"pby_dev","server_url":"https://dev.example","future":7}"#,
    )
    .expect("profile config");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_TOKEN", "pby_env_must_not_affect_logout");
    let cli = Cli::try_parse_from(["patchbay", "--profile", "dev", "auth", "logout"])
        .expect("logout CLI");
    let output = run_auth_logout(&cli, &environment).expect("logout");
    assert_eq!(output.stderr, "Token removed. You are now logged out.\n");
    let saved: Value = serde_json::from_slice(&fs::read(&profile_path).expect("saved profile"))
        .expect("profile JSON");
    assert!(saved.get("token").is_none());
    assert_eq!(saved["future"], 7);
    assert_eq!(
        fs::read(&default_path).expect("default unchanged"),
        default_bytes
    );
    assert_eq!(
        run_auth_logout(&cli, &environment)
            .expect("idempotent logout")
            .stderr,
        "Not authenticated.\n"
    );

    environment.set("PATCHBAY_AGENT_ID", "agent-1");
    assert!(run_auth_logout(&cli, &environment)
        .expect_err("task logout rejected")
        .to_string()
        .contains("not available inside a daemon-managed task"));
}
