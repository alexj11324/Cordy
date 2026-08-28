use super::*;
use axum::extract::Request;
use axum::routing::{delete as delete_route, get, post, put};
use axum::{Json, Router};
use clap::Parser;
use std::fs;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[test]
fn agent_read_parser_matches_go_registry() {
    let list = Cli::try_parse_from([
        "patchbay",
        "agent",
        "list",
        "--include-archived",
        "--output",
        "json",
    ])
    .expect("agent list CLI");
    let Command::Agent(AgentArgs {
        command: AgentCommand::List {
            output,
            include_archived,
        },
    }) = list.command
    else {
        panic!("expected agent list");
    };
    assert_eq!(output, OutputFormat::Json);
    assert!(include_archived);

    let get =
        Cli::try_parse_from(["patchbay", "agent", "get", "agent-123"]).expect("agent get CLI");
    let Command::Agent(AgentArgs {
        command: AgentCommand::Get { id, output },
    }) = get.command
    else {
        panic!("expected agent get");
    };
    assert_eq!(id, "agent-123");
    assert_eq!(output, OutputFormat::Json);
    assert!(Cli::try_parse_from(["patchbay", "agent", "list", "--full-id"]).is_err());
}

#[tokio::test]
async fn agent_list_and_get_match_go_requests_and_outputs() {
    let app = Router::new()
        .route(
            "/api/agents",
            get(|request: Request| async move {
                assert_eq!(
                    request.uri().query(),
                    Some("workspace_id=workspace-1&include_archived=true")
                );
                Json(vec![serde_json::json!({
                    "id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                    "name":"Builder",
                    "status":"active",
                    "runtime_mode":"cloud",
                    "archived_at":"2026-08-24T00:00:00Z",
                    "server_only":"preserved"
                })])
            }),
        )
        .route(
            "/api/agents/agent-123",
            get(|| async {
                Json(serde_json::json!({
                    "id":"agent-123",
                    "name":"Reviewer",
                    "status":"idle",
                    "runtime_mode":"local",
                    "visibility":"workspace",
                    "avatar_url":"https://cdn.example/avatar.png",
                    "description":"Reviews changes",
                    "server_only":"preserved"
                }))
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");

    let list = Cli::try_parse_from([
        "patchbay",
        "agent",
        "list",
        "--include-archived",
        "--output",
        "table",
    ])
    .expect("agent list CLI");
    let listed = run_with_input(&list, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list agents");
    assert!(listed.stdout.starts_with("ID"));
    assert!(listed
        .stdout
        .contains("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"));
    assert!(listed.stdout.contains("Builder"));
    assert!(listed.stdout.contains("cloud"));
    assert!(listed.stdout.contains("yes"));

    let get = Cli::try_parse_from(["patchbay", "agent", "get", "agent-123", "--output", "table"])
        .expect("agent get CLI");
    let details = run_with_input(&get, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("get agent");
    assert!(details.stdout.contains("AVATAR_URL"));
    assert!(details.stdout.contains("https://cdn.example/avatar.png"));
    assert!(details.stdout.contains("Reviews changes"));

    let get_json =
        Cli::try_parse_from(["patchbay", "agent", "get", "agent-123"]).expect("agent get JSON CLI");
    let json = run_with_input(&get_json, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("get agent JSON");
    assert_eq!(
        serde_json::from_str::<Value>(&json.stdout).expect("JSON")["server_only"],
        "preserved"
    );
    server.abort();
}

#[tokio::test]
async fn agent_list_requires_workspace_before_request() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", "http://127.0.0.1:9");
    environment.set("PATCHBAY_TOKEN", "token-1");
    let cli = Cli::try_parse_from(["patchbay", "agent", "list"]).expect("agent list CLI");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("workspace required");
    assert_eq!(
        error.to_string(),
        "workspace_id is required: use --workspace-id flag, set PATCHBAY_WORKSPACE_ID env, or run 'patchbay config set workspace_id <id>'"
    );
}

#[tokio::test]
async fn agent_create_preserves_go_request_and_secret_input_semantics() {
    let captured = Arc::new(Mutex::new(None));
    let captured_handler = Arc::clone(&captured);
    let app = Router::new().route(
        "/api/agents",
        post(move |Json(body): Json<Value>| {
            let captured = Arc::clone(&captured_handler);
            async move {
                *captured.lock().expect("captured body") = Some(body);
                Json(serde_json::json!({"id":"agent-1","name":"Builder"}))
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    fs::write(cwd.path().join("agent.env.json"), r#"{"TOKEN":"secret"}"#).expect("env file");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");
    let cli = Cli::try_parse_from([
        "patchbay",
        "agent",
        "create",
        "--name",
        "Builder",
        "--runtime-id",
        "runtime-1",
        "--description",
        "Builds things",
        "--instructions",
        "Be careful",
        "--runtime-config",
        r#"{"sandbox":true}"#,
        "--custom-args",
        r#"["--model","fast"]"#,
        "--custom-env-file",
        "agent.env.json",
        "--mcp-config-stdin",
        "--model",
        "model-1",
        "--thinking-level",
        "high",
        "--service-tier",
        "priority",
        "--visibility",
        "workspace",
        "--public-to-workspace",
        "--public-to-member",
        "user-1,user-2",
        "--max-concurrent-tasks",
        "50",
        "--output",
        "table",
    ])
    .expect("agent create CLI");
    let mut input = Cursor::new(br#"{"mcpServers":{"linear":{"token":"hidden"}}}"#.to_vec());
    let output = run_with_input(&cli, &environment, &mut input)
        .await
        .expect("create agent");
    assert_eq!(output.stdout, "Agent created: Builder (agent-1)\n");
    let body = captured
        .lock()
        .expect("captured body")
        .clone()
        .expect("request body");
    assert_eq!(body["name"], "Builder");
    assert_eq!(body["runtime_id"], "runtime-1");
    assert_eq!(body["runtime_config"]["sandbox"], true);
    assert_eq!(body["custom_args"], serde_json::json!(["--model", "fast"]));
    assert_eq!(body["custom_env"]["TOKEN"], "secret");
    assert_eq!(
        body["mcp_config"]["mcpServers"]["linear"]["token"],
        "hidden"
    );
    assert_eq!(body["model"], "model-1");
    assert_eq!(body["thinking_level"], "high");
    assert_eq!(body["service_tier"], "priority");
    assert_eq!(body["visibility"], "workspace");
    assert_eq!(body["permission_mode"], "public_to");
    assert_eq!(
        body["invocation_targets"],
        serde_json::json!([
            {"target_type":"workspace"},
            {"target_type":"member","target_id":"user-1"},
            {"target_type":"member","target_id":"user-2"}
        ])
    );
    assert_eq!(body["max_concurrent_tasks"], 50);
    server.abort();
}

#[test]
fn agent_create_rejects_invalid_and_ambiguous_secret_inputs_without_leaking() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let secret = "sk-do-not-echo";
    let invalid = Cli::try_parse_from([
        "patchbay",
        "agent",
        "create",
        "--name",
        "Builder",
        "--runtime-id",
        "runtime-1",
        "--custom-env",
        &format!(r#"{{"TOKEN":"{secret}""#),
    ])
    .expect("invalid secret CLI");
    let Command::Agent(AgentArgs {
        command: AgentCommand::Create(args),
    }) = &invalid.command
    else {
        panic!("expected agent create");
    };
    let error = resolve_agent_secret_json(
        args.custom_env.as_deref(),
        args.custom_env_stdin,
        args.custom_env_file.as_deref(),
        "custom-env",
        false,
        &environment,
        &mut Cursor::new(Vec::<u8>::new()),
    )
    .expect_err("invalid custom env");
    assert!(error.to_string().contains("valid JSON object"));
    assert!(!error.to_string().contains(secret));

    let ambiguous = Cli::try_parse_from([
        "patchbay",
        "agent",
        "create",
        "--name",
        "Builder",
        "--runtime-id",
        "runtime-1",
        "--mcp-config",
        "{}",
        "--mcp-config-stdin",
    ])
    .expect("ambiguous MCP CLI");
    let Command::Agent(AgentArgs {
        command: AgentCommand::Create(args),
    }) = &ambiguous.command
    else {
        panic!("expected agent create");
    };
    assert!(resolve_agent_secret_json(
        args.mcp_config.as_deref(),
        args.mcp_config_stdin,
        args.mcp_config_file.as_deref(),
        "mcp-config",
        true,
        &environment,
        &mut Cursor::new(b"{}".to_vec()),
    )
    .expect_err("ambiguous MCP inputs")
    .to_string()
    .contains("mutually exclusive"));
}

#[test]
fn agent_create_validates_required_fields_and_concurrency() {
    let missing_name =
        Cli::try_parse_from(["patchbay", "agent", "create", "--runtime-id", "runtime-1"])
            .expect("missing name parses for Go-compatible runtime validation");
    let Command::Agent(AgentArgs {
        command: AgentCommand::Create(args),
    }) = &missing_name.command
    else {
        panic!("expected agent create");
    };
    assert!(args.name.is_none());

    let invalid = Cli::try_parse_from([
        "patchbay",
        "agent",
        "create",
        "--name",
        "Builder",
        "--runtime-id",
        "runtime-1",
        "--max-concurrent-tasks",
        "51",
    ])
    .expect("invalid concurrency parses for runtime validation");
    let Command::Agent(AgentArgs {
        command: AgentCommand::Create(args),
    }) = &invalid.command
    else {
        panic!("expected agent create");
    };
    assert_eq!(args.max_concurrent_tasks, Some(51));
}

#[tokio::test]
async fn agent_update_puts_only_changed_fields_and_supports_mcp_clear() {
    let captured = Arc::new(Mutex::new(None));
    let captured_handler = Arc::clone(&captured);
    let app = Router::new().route(
        "/api/agents/agent-1",
        put(move |Json(body): Json<Value>| {
            let captured = Arc::clone(&captured_handler);
            async move {
                *captured.lock().expect("captured body") = Some(body);
                Json(serde_json::json!({"id":"agent-1","name":"Builder v2"}))
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
    environment.set("PATCHBAY_TOKEN", "token-1");
    let cli = Cli::try_parse_from([
        "patchbay",
        "agent",
        "update",
        "agent-1",
        "--name",
        "Builder v2",
        "--thinking-level",
        "",
        "--mcp-config",
        "null",
        "--permission-mode",
        "private",
        "--output",
        "table",
    ])
    .expect("agent update CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("update agent");
    assert_eq!(output.stdout, "Agent updated: Builder v2 (agent-1)\n");
    assert_eq!(
        captured
            .lock()
            .expect("captured body")
            .clone()
            .expect("body"),
        serde_json::json!({
            "name":"Builder v2",
            "thinking_level":"",
            "mcp_config":null,
            "permission_mode":"private",
            "invocation_targets":[]
        })
    );
    server.abort();
}

#[tokio::test]
async fn agent_update_rejects_no_changes_and_does_not_expose_custom_env() {
    assert!(Cli::try_parse_from([
        "patchbay",
        "agent",
        "update",
        "agent-1",
        "--custom-env",
        "{}"
    ])
    .is_err());
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", "http://127.0.0.1:9");
    environment.set("PATCHBAY_TOKEN", "token-1");
    let cli =
        Cli::try_parse_from(["patchbay", "agent", "update", "agent-1"]).expect("agent update CLI");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("no changes");
    assert!(error.to_string().contains("no fields to update"));
    assert!(error.to_string().contains("patchbay agent env set <id>"));
}

#[tokio::test]
async fn agent_lifecycle_and_tasks_match_go_requests_and_outputs() {
    let app = Router::new()
        .route(
            "/api/agents/agent-1/archive",
            post(|Json(body): Json<Value>| async move {
                assert!(body.is_null());
                Json(serde_json::json!({"id":"agent-1","name":"Builder","archived_at":"now"}))
            }),
        )
        .route(
            "/api/agents/agent-1/restore",
            post(|Json(body): Json<Value>| async move {
                assert!(body.is_null());
                Json(serde_json::json!({"id":"agent-1","name":"Builder","archived_at":null}))
            }),
        )
        .route(
            "/api/agents/agent-1/tasks",
            get(|| async {
                Json(vec![serde_json::json!({
                    "id":"task-1",
                    "issue_id":"issue-1",
                    "status":"completed",
                    "created_at":"2026-08-24T00:00:00Z",
                    "server_only":"preserved"
                })])
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_TOKEN", "token-1");

    for (command, expected) in [
        ("archive", "Agent archived: Builder (agent-1)\n"),
        ("restore", "Agent restored: Builder (agent-1)\n"),
    ] {
        let cli =
            Cli::try_parse_from(["patchbay", "agent", command, "agent-1", "--output", "table"])
                .expect("agent lifecycle CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("agent lifecycle request");
        assert_eq!(output.stdout, expected);
    }

    let tasks =
        Cli::try_parse_from(["patchbay", "agent", "tasks", "agent-1"]).expect("agent tasks CLI");
    let table = run_with_input(&tasks, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list agent tasks");
    assert!(table.stdout.starts_with("ID"));
    assert!(table.stdout.contains("task-1"));
    assert!(table.stdout.contains("issue-1"));
    assert!(table.stdout.contains("completed"));

    let tasks_json =
        Cli::try_parse_from(["patchbay", "agent", "tasks", "agent-1", "--output", "json"])
            .expect("agent tasks JSON CLI");
    let json = run_with_input(
        &tasks_json,
        &environment,
        &mut Cursor::new(Vec::<u8>::new()),
    )
    .await
    .expect("list agent tasks JSON");
    assert_eq!(
        serde_json::from_str::<Value>(&json.stdout).expect("JSON")[0]["server_only"],
        "preserved"
    );
    server.abort();
}

#[tokio::test]
async fn agent_avatar_prechecks_uploads_and_updates_agent() {
    let app = Router::new()
        .route(
            "/api/agents/agent-1",
            get(|| async { Json(serde_json::json!({"id":"agent-1","name":"Builder"})) }).put(
                |Json(body): Json<Value>| async move {
                    assert_eq!(body["avatar_url"], "https://cdn.example/avatar.png");
                    Json(serde_json::json!({"id":"agent-1","avatar_url":body["avatar_url"]}))
                },
            ),
        )
        .route(
            "/api/upload-file",
            post(|request: Request| async move {
                assert!(request
                    .headers()
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| value.starts_with("multipart/form-data; boundary=")));
                let bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                    .await
                    .expect("multipart body");
                let body = String::from_utf8_lossy(&bytes);
                assert!(body.contains("filename=\"avatar.PNG\""));
                assert!(body.contains("fake-png-data"));
                Json(serde_json::json!({
                    "id":"attachment-1",
                    "url":"https://cdn.example/avatar.png"
                }))
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    fs::write(cwd.path().join("avatar.PNG"), b"fake-png-data").expect("avatar");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_TOKEN", "token-1");
    let cli = Cli::try_parse_from([
        "patchbay",
        "agent",
        "avatar",
        "agent-1",
        "--file",
        "avatar.PNG",
        "--output",
        "table",
    ])
    .expect("agent avatar CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("upload avatar");
    assert!(output.stdout.starts_with("ID"));
    assert!(output.stdout.contains("attachment-1"));
    assert!(output.stdout.contains("agent-1"));
    assert!(output.stdout.contains("https://cdn.example/avatar.png"));
    server.abort();
}

#[tokio::test]
async fn agent_avatar_rejects_missing_bad_and_oversized_files_before_api_calls() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    fs::write(cwd.path().join("avatar.txt"), b"not an image").expect("bad avatar");
    fs::write(cwd.path().join("large.png"), vec![0; (5 << 20) + 1]).expect("large avatar");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", "http://127.0.0.1:9");
    environment.set("PATCHBAY_TOKEN", "token-1");
    for (args, message) in [
        (
            vec!["patchbay", "agent", "avatar", "agent-1"],
            "--file is required",
        ),
        (
            vec![
                "patchbay",
                "agent",
                "avatar",
                "agent-1",
                "--file",
                "avatar.txt",
            ],
            "unsupported file format",
        ),
        (
            vec![
                "patchbay",
                "agent",
                "avatar",
                "agent-1",
                "--file",
                "large.png",
            ],
            "file too large",
        ),
    ] {
        let cli = Cli::try_parse_from(args).expect("agent avatar CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("avatar validation");
        assert!(error.to_string().contains(message), "{error:#}");
    }
}

#[tokio::test]
async fn agent_skills_list_set_and_add_match_go_contract() {
    let app = Router::new()
        .route(
            "/api/agents/agent-1/skills",
            get(|| async {
                Json(vec![serde_json::json!({
                    "id":"skill-1","name":"Review","description":"Reviews code"
                })])
            })
            .put(|Json(body): Json<Value>| async move {
                assert_eq!(body, serde_json::json!({"skill_ids":[]}));
                Json(Vec::<Value>::new())
            }),
        )
        .route(
            "/api/agents/agent-1/skills/add",
            post(|Json(body): Json<Value>| async move {
                assert_eq!(body, serde_json::json!({"skill_ids":["skill-1","skill-2"]}));
                Json(vec![serde_json::json!({
                    "id":"skill-1","name":"Review","description":"Reviews code",
                    "server_only":"preserved"
                })])
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_TOKEN", "token-1");

    let list = Cli::try_parse_from(["patchbay", "agent", "skills", "list", "agent-1"])
        .expect("agent skills list CLI");
    let listed = run_with_input(&list, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list skills");
    assert!(listed.stdout.starts_with("ID"));
    assert!(listed.stdout.contains("Reviews code"));

    let set = Cli::try_parse_from([
        "patchbay",
        "agent",
        "skills",
        "set",
        "agent-1",
        "--skill-ids",
        "",
        "--output",
        "table",
    ])
    .expect("agent skills clear CLI");
    let cleared = run_with_input(&set, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("clear skills");
    assert_eq!(cleared.stdout, "No skills assigned to agent agent-1\n");

    let add = Cli::try_parse_from([
        "patchbay",
        "agent",
        "skills",
        "add",
        "agent-1",
        "--skill-ids",
        " skill-1,skill-2 ",
    ])
    .expect("agent skills add CLI");
    let added = run_with_input(&add, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("add skills");
    assert_eq!(
        serde_json::from_str::<Value>(&added.stdout).expect("JSON")[0]["server_only"],
        "preserved"
    );
    server.abort();
}

#[tokio::test]
async fn agent_skills_mutations_enforce_go_skill_id_requirements() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", "http://127.0.0.1:9");
    environment.set("PATCHBAY_TOKEN", "token-1");
    for (command, skill_ids, expected) in [
        ("set", None, "--skill-ids is required"),
        ("add", None, "--skill-ids is required"),
        (
            "add",
            Some(" , "),
            "--skill-ids must include at least one skill ID",
        ),
    ] {
        let mut argv = vec!["patchbay", "agent", "skills", command, "agent-1"];
        if let Some(skill_ids) = skill_ids {
            argv.extend(["--skill-ids", skill_ids]);
        }
        let cli = Cli::try_parse_from(argv).expect("agent skills mutation CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("skill IDs required");
        assert!(error.to_string().contains(expected), "{error:#}");
    }
}

#[tokio::test]
async fn agent_env_get_and_set_use_audited_endpoint_and_preserve_values() {
    let app = Router::new().route(
        "/api/agents/agent-1/env",
        get(|| async {
            Json(serde_json::json!({
                "custom_env":{"API_KEY":"plaintext","COUNT":"2"}
            }))
        })
        .put(|Json(body): Json<Value>| async move {
            assert_eq!(
                body,
                serde_json::json!({"custom_env":{"API_KEY":"****","NEW":"value"}})
            );
            Json(serde_json::json!({
                "custom_env":{"API_KEY":"plaintext","NEW":"value"}
            }))
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_TOKEN", "token-1");

    let get = Cli::try_parse_from([
        "patchbay", "agent", "env", "get", "agent-1", "--output", "table",
    ])
    .expect("agent env get CLI");
    let env = run_with_input(&get, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("get agent env");
    assert!(env.stdout.starts_with("KEY"));
    assert!(env.stdout.contains("API_KEY"));
    assert!(env.stdout.contains("plaintext"));

    let set = Cli::try_parse_from([
        "patchbay",
        "agent",
        "env",
        "set",
        "agent-1",
        "--custom-env-stdin",
        "--output",
        "table",
    ])
    .expect("agent env set CLI");
    let updated = run_with_input(
        &set,
        &environment,
        &mut Cursor::new(br#"{"API_KEY":"****","NEW":"value"}"#.to_vec()),
    )
    .await
    .expect("set agent env");
    assert_eq!(updated.stdout, "Env updated for agent agent-1 (2 keys)\n");
    server.abort();
}

#[tokio::test]
async fn agent_env_set_requires_one_secret_safe_input_channel() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", "http://127.0.0.1:9");
    environment.set("PATCHBAY_TOKEN", "token-1");
    let cli = Cli::try_parse_from(["patchbay", "agent", "env", "set", "agent-1"])
        .expect("agent env set CLI");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("env input required");
    assert!(error
        .to_string()
        .contains("specify the new env via --custom-env"));
}

#[test]
fn agent_mcp_paths_trim_and_escape_each_identifier() {
    assert_eq!(
        agent_mcp_path(" agent/one ", &["server/two", "enabled"]),
        "/api/agents/agent%2Fone/mcp-servers/server%2Ftwo/enabled"
    );
}

#[tokio::test]
async fn agent_mcp_commands_match_go_api_and_redacted_output_contract() {
    let server_value = || {
        serde_json::json!({
            "id":"server-1","name":"linear","transport":"http","enabled":true,
            "config":{"headers":{"Authorization":"secret"}}
        })
    };
    let app = Router::new()
        .route(
            "/api/agents/agent-1/mcp-servers",
            get({
                let value = server_value();
                move || {
                    let value = value.clone();
                    async move { Json(vec![value]) }
                }
            })
            .post({
                let value = server_value();
                move |Json(body): Json<Value>| {
                    let value = value.clone();
                    async move {
                        assert_eq!(body, serde_json::json!({"server_id":"server-1"}));
                        Json(vec![value])
                    }
                }
            }),
        )
        .route(
            "/api/agents/agent-1/mcp-servers/server-1/enabled",
            put({
                let value = server_value();
                move |Json(body): Json<Value>| {
                    let value = value.clone();
                    async move {
                        assert!(body.get("enabled").and_then(Value::as_bool).is_some());
                        Json(vec![value])
                    }
                }
            }),
        )
        .route(
            "/api/agents/agent-1/mcp-servers/server-1",
            delete_route(|| async { Json(Vec::<Value>::new()) }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_TOKEN", "token-1");

    for argv in [
        vec!["patchbay", "agent", "mcp", "list", "agent-1"],
        vec!["patchbay", "agent", "mcp", "add", "agent-1", "server-1"],
        vec!["patchbay", "agent", "mcp", "enable", "agent-1", "server-1"],
        vec!["patchbay", "agent", "mcp", "disable", "agent-1", "server-1"],
    ] {
        let cli = Cli::try_parse_from(argv).expect("agent MCP CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("agent MCP request");
        assert!(output.stdout.contains("linear"));
        assert!(output.stdout.contains("enabled"));
        assert!(!output.stdout.contains("secret"));
        assert!(!output.stdout.contains("Authorization"));
    }

    let remove = Cli::try_parse_from(["patchbay", "agent", "mcp", "remove", "agent-1", "server-1"])
        .expect("agent MCP remove CLI");
    let removed = run_with_input(&remove, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("remove agent MCP server");
    assert_eq!(removed.stdout, "no MCP servers\n");
    server.abort();
}

#[tokio::test]
async fn agent_copy_copies_only_portable_same_runtime_fields() {
    let source = serde_json::json!({
        "id":"agent-source","name":"Source","runtime_id":"runtime-1",
        "description":"description","instructions":"instructions",
        "avatar_url":"https://cdn.example/avatar.png",
        "custom_args":["--foo"],"max_concurrent_tasks":9,
        "model":"model-1","thinking_level":"high","service_tier":"priority",
        "permission_mode":"public_to",
        "invocation_targets":[{"target_type":"workspace"}],
        "skills":[{"id":"skill-1"},{"id":"skill-2"}],
        "has_custom_env":true,"custom_env_key_count":2,"mcp_config_redacted":true,
        "runtime_config":{"machine":"must-not-copy"}
    });
    let captured = Arc::new(Mutex::new(None));
    let captured_handler = Arc::clone(&captured);
    let app = Router::new()
        .route(
            "/api/agents/agent-source",
            get(move || {
                let source = source.clone();
                async move { Json(source) }
            }),
        )
        .route(
            "/api/agents",
            post(move |Json(body): Json<Value>| {
                let captured = Arc::clone(&captured_handler);
                async move {
                    *captured.lock().expect("captured body") = Some(body);
                    Json(serde_json::json!({"id":"agent-copy","name":"Source (copy)"}))
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
    environment.set("PATCHBAY_TOKEN", "token-1");
    let cli =
        Cli::try_parse_from(["patchbay", "agent", "copy", "agent-source"]).expect("agent copy CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("copy agent");
    assert_eq!(
        serde_json::from_str::<Value>(&output.stdout).expect("JSON")["id"],
        "agent-copy"
    );
    let body = captured
        .lock()
        .expect("captured body")
        .clone()
        .expect("body");
    assert_eq!(body["name"], "Source (copy)");
    assert_eq!(body["runtime_id"], "runtime-1");
    assert_eq!(body["description"], "description");
    assert_eq!(body["instructions"], "instructions");
    assert_eq!(body["avatar_url"], "https://cdn.example/avatar.png");
    assert_eq!(body["custom_args"], serde_json::json!(["--foo"]));
    assert_eq!(body["max_concurrent_tasks"], 9);
    assert_eq!(body["model"], "model-1");
    assert_eq!(body["thinking_level"], "high");
    assert_eq!(body["service_tier"], "priority");
    assert_eq!(body["permission_mode"], "public_to");
    assert_eq!(body["skill_ids"], serde_json::json!(["skill-1", "skill-2"]));
    for forbidden in [
        "custom_env",
        "mcp_config",
        "runtime_config",
        "has_custom_env",
        "custom_env_key_count",
        "mcp_config_redacted",
    ] {
        assert!(body.get(forbidden).is_none(), "copied {forbidden}");
    }
    server.abort();
}

#[tokio::test]
async fn agent_copy_cross_runtime_requires_model_and_drops_runtime_fields() {
    let posts = Arc::new(Mutex::new(Vec::<Value>::new()));
    let posts_handler = Arc::clone(&posts);
    let source = serde_json::json!({
        "id":"agent-source","name":"Source","runtime_id":"runtime-1",
        "model":"old-model","thinking_level":"high","service_tier":"priority",
        "max_concurrent_tasks":0
    });
    let app = Router::new()
        .route(
            "/api/agents/agent-source",
            get(move || {
                let source = source.clone();
                async move { Json(source) }
            }),
        )
        .route(
            "/api/agents",
            post(move |Json(body): Json<Value>| {
                let posts = Arc::clone(&posts_handler);
                async move {
                    posts.lock().expect("posts").push(body);
                    Json(serde_json::json!({"id":"agent-copy","name":"Source (copy)"}))
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
    environment.set("PATCHBAY_TOKEN", "token-1");

    let missing_model = Cli::try_parse_from([
        "patchbay",
        "agent",
        "copy",
        "agent-source",
        "--runtime-id",
        "runtime-2",
    ])
    .expect("cross-runtime copy CLI");
    let error = run_with_input(
        &missing_model,
        &environment,
        &mut Cursor::new(Vec::<u8>::new()),
    )
    .await
    .expect_err("model required");
    assert!(error.to_string().contains("requires --model"));
    assert!(posts.lock().expect("posts").is_empty());

    let copy = Cli::try_parse_from([
        "patchbay",
        "agent",
        "copy",
        "agent-source",
        "--runtime-id",
        "runtime-2",
        "--model",
        "",
        "--no-skills",
    ])
    .expect("cross-runtime copy with model CLI");
    run_with_input(&copy, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("copy across runtime");
    let body = posts.lock().expect("posts")[0].clone();
    assert_eq!(body["runtime_id"], "runtime-2");
    assert_eq!(body["model"], "");
    assert!(body.get("thinking_level").is_none());
    assert!(body.get("service_tier").is_none());
    assert!(body.get("max_concurrent_tasks").is_none());
    assert!(body.get("skill_ids").is_none());
    server.abort();
}
