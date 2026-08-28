use super::*;
use axum::extract::Request;
use axum::routing::{delete as delete_route, get, patch, post};
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use std::sync::Mutex;
use tokio::net::TcpListener;

#[tokio::test]
async fn runtime_read_commands_match_go_requests_and_tables() {
    let app = Router::new()
        .route(
            "/api/runtimes",
            get(|| async {
                Json(vec![serde_json::json!({
                    "id":"runtime-1","name":"Mac","runtime_mode":"local",
                    "provider":"codex","status":"online","last_seen_at":"now",
                    "server_only":"preserved"
                })])
            }),
        )
        .route(
            "/api/runtimes/runtime-1/usage",
            get(|request: Request| async move {
                assert_eq!(request.uri().query(), Some("days=30"));
                Json(vec![serde_json::json!({
                    "date":"2026-08-24","provider":"codex","model":"gpt",
                    "input_tokens":10,"output_tokens":5,
                    "cache_read_tokens":2,"cache_write_tokens":1
                })])
            }),
        )
        .route(
            "/api/runtimes/runtime-1/activity",
            get(|| async { Json(vec![serde_json::json!({"hour":"12","count":3})]) }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_TOKEN", "token-1");

    let list = Cli::try_parse_from(["cordy", "runtime", "list"]).expect("runtime list CLI");
    let listed = run_with_input(&list, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list runtimes");
    assert!(listed.stdout.starts_with("ID"));
    assert!(listed.stdout.contains("runtime-1"));
    assert!(listed.stdout.contains("codex"));

    let usage = Cli::try_parse_from(["cordy", "runtime", "usage", "runtime-1", "--days", "30"])
        .expect("runtime usage CLI");
    let used = run_with_input(&usage, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("runtime usage");
    assert!(used.stdout.starts_with("DATE"));
    assert!(used.stdout.contains("2026-08-24"));
    assert!(used.stdout.contains("10"));

    let activity = Cli::try_parse_from([
        "cordy",
        "runtime",
        "activity",
        "runtime-1",
        "--output",
        "json",
    ])
    .expect("runtime activity CLI");
    let active = run_with_input(&activity, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("runtime activity");
    assert_eq!(
        serde_json::from_str::<Value>(&active.stdout).expect("JSON")[0]["count"],
        3
    );
    server.abort();
}

#[tokio::test]
async fn runtime_usage_rejects_days_outside_go_range() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", "http://127.0.0.1:9");
    environment.set("CORDY_TOKEN", "token-1");
    for days in ["0", "366"] {
        let cli = Cli::try_parse_from(["cordy", "runtime", "usage", "runtime-1", "--days", days])
            .expect("runtime usage CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("days range");
        assert_eq!(error.to_string(), "--days must be between 1 and 365");
    }
}

#[tokio::test]
async fn runtime_rename_and_cascade_delete_match_go_contract() {
    let app = Router::new()
        .route(
            "/api/runtimes/runtime-1",
            patch(|Json(body): Json<Value>| async move {
                assert_eq!(
                    body,
                    serde_json::json!({"custom_name":"Build Mac","apply_to_machine":true})
                );
                Json(serde_json::json!({
                    "id":"runtime-1","name":"Build Mac","custom_name":"Build Mac"
                }))
            })
            .delete(|| async {
                (
                    axum::http::StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "code":"runtime_has_active_agents",
                        "error":"runtime has active agents",
                        "active_agents":[
                            {"id":"agent-1","name":"Builder"},
                            {"id":"agent-2","name":""}
                        ]
                    })),
                )
            }),
        )
        .route(
            "/api/runtimes/runtime-1/unbind-agents-and-delete",
            post(|Json(body): Json<Value>| async move {
                assert_eq!(
                    body,
                    serde_json::json!({"expected_active_agent_ids":["agent-1","agent-2"]})
                );
                Json(serde_json::json!({
                    "agents_unbound":2,"autopilots_paused":1
                }))
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_TOKEN", "token-1");

    let rename = Cli::try_parse_from([
        "cordy",
        "runtime",
        "rename",
        "runtime-1",
        "Build Mac",
        "--machine",
    ])
    .expect("runtime rename CLI");
    let renamed = run_with_input(&rename, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("rename runtime");
    assert!(renamed.stdout.is_empty());
    assert_eq!(renamed.stderr, "Runtime renamed to \"Build Mac\".\n");

    let delete = Cli::try_parse_from(["cordy", "runtime", "delete", "runtime-1"])
        .expect("runtime delete CLI");
    let conflict = run_with_input(&delete, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("active agents conflict");
    assert!(conflict.to_string().contains("Builder (agent-1), agent-2"));
    assert!(conflict.to_string().contains("--cascade"));

    let cascade = Cli::try_parse_from(["cordy", "runtime", "delete", "runtime-1", "--cascade"])
        .expect("runtime cascade delete CLI");
    let deleted = run_with_input(&cascade, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("cascade delete runtime");
    assert!(deleted.stdout.is_empty());
    assert_eq!(
        deleted.stderr,
        "Runtime runtime-1 deleted; unbound 2 agent(s) and paused 1 autopilot(s).\n"
    );
    server.abort();
}

#[tokio::test]
async fn runtime_delete_strict_success_returns_go_json_mirror() {
    let app = Router::new().route(
        "/api/runtimes/runtime-1",
        delete_route(|| async { axum::http::StatusCode::NO_CONTENT }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_TOKEN", "token-1");
    let cli = Cli::try_parse_from([
        "cordy",
        "runtime",
        "delete",
        "runtime-1",
        "--output",
        "json",
    ])
    .expect("runtime delete JSON CLI");
    let deleted = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("delete runtime");
    assert_eq!(
        serde_json::from_str::<Value>(&deleted.stdout).expect("JSON"),
        serde_json::json!({"id":"runtime-1","deleted":true})
    );
    server.abort();
}

#[tokio::test]
async fn runtime_update_initiates_and_waits_with_injected_poll_policy() {
    let polls = Arc::new(Mutex::new(0usize));
    let polls_handler = Arc::clone(&polls);
    let app = Router::new()
        .route(
            "/api/runtimes/runtime-1/update",
            post(|Json(body): Json<Value>| async move {
                assert_eq!(body, serde_json::json!({"target_version":"v2.0.0"}));
                Json(serde_json::json!({"id":"update-1","status":"pending"}))
            }),
        )
        .route(
            "/api/runtimes/runtime-1/update/update-1",
            get(move || {
                let polls = Arc::clone(&polls_handler);
                async move {
                    let mut count = polls.lock().expect("poll count");
                    *count += 1;
                    if *count == 1 {
                        Json(serde_json::json!({"id":"update-1","status":"running"}))
                    } else {
                        Json(serde_json::json!({
                            "id":"update-1","status":"completed","output":"updated"
                        }))
                    }
                }
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_TOKEN", "token-1");
    let cli = Cli::try_parse_from([
        "cordy",
        "runtime",
        "update",
        "runtime-1",
        "--target-version",
        "v2.0.0",
        "--wait",
        "--output",
        "table",
    ])
    .expect("runtime update CLI");
    let Command::Runtime(RuntimeArgs {
        command:
            RuntimeCommand::Update {
                runtime_id,
                target_version,
                output,
                wait,
            },
    }) = &cli.command
    else {
        panic!("expected runtime update");
    };
    let updated = run_runtime_update_with_policy(
        &cli,
        &environment,
        runtime_id,
        target_version.as_deref(),
        *output,
        *wait,
        Duration::from_millis(1),
        Duration::from_secs(1),
    )
    .await
    .expect("wait for runtime update");
    assert_eq!(updated.stdout, "Update completed: updated\n");
    assert_eq!(*polls.lock().expect("poll count"), 2);
    server.abort();
}

#[tokio::test]
async fn runtime_update_timeout_reports_last_status() {
    let app = Router::new()
        .route(
            "/api/runtimes/runtime-1/update",
            post(|| async { Json(serde_json::json!({"id":"update-1","status":"pending"})) }),
        )
        .route(
            "/api/runtimes/runtime-1/update/update-1",
            get(|| async { Json(serde_json::json!({"id":"update-1","status":"running"})) }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_TOKEN", "token-1");
    let cli = Cli::try_parse_from([
        "cordy",
        "runtime",
        "update",
        "runtime-1",
        "--target-version",
        "v2",
        "--wait",
    ])
    .expect("runtime update CLI");
    let error = run_runtime_update_with_policy(
        &cli,
        &environment,
        "runtime-1",
        Some("v2"),
        OutputFormat::Json,
        true,
        Duration::from_millis(1),
        Duration::from_millis(10),
    )
    .await
    .expect_err("runtime update timeout");
    assert!(error
        .to_string()
        .starts_with("timed out waiting for update (last status:"));
    server.abort();
}

#[test]
fn runtime_update_terminal_table_outputs_match_go() {
    assert_eq!(
        format_runtime_update_result(
            &serde_json::json!({"status":"failed","error":"boom"}),
            OutputFormat::Table,
            true,
        )
        .expect("failed update output")
        .stdout,
        "Update failed: boom\n"
    );
    assert_eq!(
        format_runtime_update_result(
            &serde_json::json!({"status":"timeout","error":"daemon timeout"}),
            OutputFormat::Table,
            true,
        )
        .expect("timeout update output")
        .stdout,
        "Update timeout: daemon timeout\n"
    );
}

#[tokio::test]
async fn runtime_profile_registry_commands_match_go_contract() {
    let collection = "/api/workspaces/workspace-1/runtime-profiles";
    let resource = "/api/workspaces/workspace-1/runtime-profiles/profile-1";
    let app = Router::new()
        .route(
            collection,
            get(|| async {
                Json(serde_json::json!({"runtime_profiles":[
                    {"id":"profile-2","display_name":"Zulu","protocol_family":"codex","command_name":"z","enabled":true},
                    {"id":"profile-1","display_name":"Alpha","protocol_family":"claude","command_name":"a","enabled":false}
                ]}))
            })
            .post(|Json(body): Json<Value>| async move {
                assert_eq!(body["protocol_family"], "codex");
                assert_eq!(body["command_name"], "wrapper");
                assert_eq!(body["display_name"], "Wrapper");
                assert!(body.get("description").is_none());
                Json(serde_json::json!({
                    "id":"profile-1","display_name":"Wrapper","protocol_family":"codex",
                    "command_name":"wrapper","enabled":true,"server_only":"preserved"
                }))
            }),
        )
        .route(
            resource,
            patch(|Json(body): Json<Value>| async move {
                assert_eq!(body, serde_json::json!({"description":"","enabled":false}));
                Json(serde_json::json!({
                    "id":"profile-1","display_name":"Wrapper","protocol_family":"codex",
                    "command_name":"wrapper","description":"","enabled":false
                }))
            })
            .delete(|| async { axum::http::StatusCode::NO_CONTENT }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");
    environment.set("CORDY_TOKEN", "token-1");

    let list = Cli::try_parse_from(["cordy", "runtime", "profile", "list"])
        .expect("runtime profile list CLI");
    let listed = run_with_input(&list, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list runtime profiles");
    assert!(
        listed.stdout.find("Alpha").expect("Alpha") < listed.stdout.find("Zulu").expect("Zulu")
    );

    let create = Cli::try_parse_from([
        "cordy",
        "runtime",
        "profile",
        "create",
        "--protocol-family",
        "codex",
        "--command-name",
        "wrapper",
        "--display-name",
        "Wrapper",
    ])
    .expect("runtime profile create CLI");
    let created = run_with_input(&create, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("create runtime profile");
    assert_eq!(
        serde_json::from_str::<Value>(&created.stdout).expect("JSON")["server_only"],
        "preserved"
    );

    let update = Cli::try_parse_from([
        "cordy",
        "runtime",
        "profile",
        "update",
        "profile-1",
        "--description",
        "",
        "--enabled=false",
    ])
    .expect("runtime profile update CLI");
    run_with_input(&update, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("update runtime profile");

    let delete = Cli::try_parse_from(["cordy", "runtime", "profile", "delete", "profile-1"])
        .expect("runtime profile delete CLI");
    let deleted = run_with_input(&delete, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("delete runtime profile");
    assert_eq!(deleted.stdout, "Deleted runtime profile profile-1\n");
    server.abort();
}

#[tokio::test]
async fn runtime_profile_validates_create_update_and_delete_conflict() {
    let app = Router::new().route(
        "/api/workspaces/workspace-1/runtime-profiles/profile-1",
        delete_route(|| async {
            (
                axum::http::StatusCode::CONFLICT,
                "active agents remain bound",
            )
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");
    environment.set("CORDY_TOKEN", "token-1");

    let invalid = Cli::try_parse_from([
        "cordy",
        "runtime",
        "profile",
        "create",
        "--protocol-family",
        "unknown",
        "--command-name",
        "wrapper",
        "--display-name",
        "Wrapper",
    ])
    .expect("invalid family parses for runtime validation");
    let error = run_with_input(&invalid, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("invalid protocol family");
    assert!(error.to_string().contains("must be one of"));

    let empty_update = Cli::try_parse_from(["cordy", "runtime", "profile", "update", "profile-1"])
        .expect("empty runtime profile update CLI");
    let error = run_with_input(
        &empty_update,
        &environment,
        &mut Cursor::new(Vec::<u8>::new()),
    )
    .await
    .expect_err("no fields");
    assert!(error.to_string().contains("no fields to update"));

    let delete = Cli::try_parse_from(["cordy", "runtime", "profile", "delete", "profile-1"])
        .expect("runtime profile delete CLI");
    let error = run_with_input(&delete, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("profile conflict");
    assert_eq!(
        error.to_string(),
        "cannot delete runtime profile profile-1: active agents remain bound"
    );
    server.abort();
}

#[tokio::test]
async fn runtime_profile_path_overrides_are_locked_atomic_and_profile_local() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let profile_dir = home.path().join(".cordy/profiles/dev");
    fs::create_dir_all(&profile_dir).expect("profile dir");
    fs::write(
        profile_dir.join("config.json"),
        r#"{"server_url":"https://api.example","unknown":{"keep":true}}"#,
    )
    .expect("profile config");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let set = Cli::try_parse_from([
        "cordy",
        "--profile",
        "dev",
        "runtime",
        "profile",
        "set-path",
        "profile-1",
        "--path",
        "/opt/bin/company-codex",
    ])
    .expect("runtime profile set-path CLI");
    let output = run_with_input(&set, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("set profile path");
    assert!(output.stdout.contains("Pinned runtime profile profile-1"));
    let document: Value =
        serde_json::from_slice(&fs::read(profile_dir.join("config.json")).expect("updated config"))
            .expect("config JSON");
    assert_eq!(document["server_url"], "https://api.example");
    assert_eq!(document["unknown"]["keep"], true);
    assert_eq!(
        document["profile_command_overrides"]["profile-1"],
        "/opt/bin/company-codex"
    );
    assert!(!home.path().join(".cordy/config.json").exists());

    let unset = Cli::try_parse_from([
        "cordy",
        "--profile",
        "dev",
        "runtime",
        "profile",
        "unset-path",
        "profile-1",
    ])
    .expect("runtime profile unset-path CLI");
    let output = run_with_input(&unset, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("unset profile path");
    assert!(output.stdout.contains("Removed per-machine path override"));
    let document: Value =
        serde_json::from_slice(&fs::read(profile_dir.join("config.json")).expect("updated config"))
            .expect("config JSON");
    assert!(document.get("profile_command_overrides").is_none());
    assert_eq!(document["unknown"]["keep"], true);

    let output = run_with_input(&unset, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("idempotent unset");
    assert_eq!(
        output.stdout,
        "No per-machine path override set for runtime profile profile-1.\n"
    );
}

#[tokio::test]
async fn runtime_profile_path_mutation_fails_closed_in_task_context() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let config_dir = home.path().join(".cordy");
    fs::create_dir_all(&config_dir).expect("config dir");
    let owner = br#"{"profile_command_overrides":{"owner":"/owner/bin"},"token":"pby_owner"}"#;
    fs::write(config_dir.join("config.json"), owner).expect("owner config");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_AGENT_ID", "agent-1");
    environment.set("CORDY_TASK_ID", "task-1");
    let cli = Cli::try_parse_from([
        "cordy",
        "runtime",
        "profile",
        "set-path",
        "profile-1",
        "--path",
        "/opt/bin/runtime",
    ])
    .expect("runtime profile set-path CLI");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("task context denied");
    assert!(error
        .to_string()
        .contains("not available inside a daemon-managed task"));
    assert_eq!(
        fs::read(config_dir.join("config.json")).expect("owner config"),
        owner
    );
}
