use super::*;
use axum::extract::Request;
use axum::http::HeaderMap;
use axum::routing::{delete as delete_route, get, patch, post};
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[test]
fn autopilot_read_parser_matches_go_registry() {
    let _list = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "list",
        "--status",
        "paused",
        "--output",
        "json",
        "--full-id",
    ])
    .expect("autopilot list CLI");
    let trigger =
        Cli::try_parse_from(["cordy", "autopilot", "trigger", "abcd", "--output", "table"])
            .expect("autopilot trigger CLI");
    let Command::Autopilot(AutopilotArgs {
        command: AutopilotCommand::Trigger { id, output },
    }) = trigger.command
    else {
        panic!("expected autopilot trigger");
    };
    assert_eq!(id, "abcd");
    assert_eq!(output, OutputFormat::Table);

    let runs = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "runs",
        "abcd",
        "--limit",
        "5",
        "--offset",
        "2",
        "--output",
        "json",
    ])
    .expect("autopilot runs CLI");
    let Command::Autopilot(AutopilotArgs {
        command:
            AutopilotCommand::Runs {
                id,
                limit,
                offset,
                output,
            },
    }) = runs.command
    else {
        panic!("expected autopilot runs");
    };
    assert_eq!(id, "abcd");
    assert_eq!(limit, 5);
    assert_eq!(offset, 2);
    assert_eq!(output, OutputFormat::Json);

    let add = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "trigger-add",
        "abcd",
        "--kind",
        "webhook",
        "--label",
        "GitHub",
    ])
    .expect("autopilot trigger-add CLI");
    let Command::Autopilot(AutopilotArgs {
        command: AutopilotCommand::TriggerAdd(args),
    }) = add.command
    else {
        panic!("expected autopilot trigger-add");
    };
    assert_eq!(args.autopilot_id, "abcd");
    assert_eq!(args.kind, "webhook");
    assert_eq!(args.label, "GitHub");
    assert_eq!(args.output, OutputFormat::Json);

    let update = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "trigger-update",
        "abcd",
        "beef",
        "--enabled=false",
        "--cron=",
        "--label=",
    ])
    .expect("autopilot trigger-update CLI");
    let Command::Autopilot(AutopilotArgs {
        command: AutopilotCommand::TriggerUpdate(args),
    }) = update.command
    else {
        panic!("expected autopilot trigger-update");
    };
    assert_eq!(args.autopilot_id, "abcd");
    assert_eq!(args.trigger_id, "beef");
    assert_eq!(args.enabled, Some(false));
    assert_eq!(args.cron.as_deref(), Some(""));
    assert_eq!(args.label.as_deref(), Some(""));

    let delete = Cli::try_parse_from(["cordy", "autopilot", "trigger-delete", "abcd", "beef"])
        .expect("autopilot trigger-delete CLI");
    assert!(matches!(
        delete.command,
        Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::TriggerDelete { .. }
        })
    ));
    assert_eq!(output, OutputFormat::Json);

    let rotate = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "trigger-rotate-url",
        "abcd",
        "beef",
        "--output",
        "table",
        "-y",
    ])
    .expect("autopilot trigger-rotate-url CLI");
    let Command::Autopilot(AutopilotArgs {
        command: AutopilotCommand::TriggerRotateUrl(args),
    }) = rotate.command
    else {
        panic!("expected autopilot trigger-rotate-url");
    };
    assert_eq!(args.autopilot_id, "abcd");
    assert_eq!(args.trigger_id, "beef");
    assert_eq!(args.output, OutputFormat::Table);
    assert!(args.yes);

    assert!(Cli::try_parse_from(["cordy", "autopilot", "get"]).is_err());
    assert!(Cli::try_parse_from(["cordy", "autopilot", "list", "extra"]).is_err());

    let create = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "create",
        "--title",
        "Daily planner",
        "--agent",
        "Planner",
        "--mode",
        "create_issue",
        "--priority",
        "high",
        "--subscriber",
        "Alice",
        "--subscriber",
        "Bob",
        "--output",
        "table",
    ])
    .expect("autopilot create CLI");
    let Command::Autopilot(AutopilotArgs {
        command: AutopilotCommand::Create(args),
    }) = create.command
    else {
        panic!("expected autopilot create");
    };
    assert_eq!(args.title.as_deref(), Some("Daily planner"));
    assert_eq!(args.agent.as_deref(), Some("Planner"));
    assert_eq!(args.mode.as_deref(), Some("create_issue"));
    assert_eq!(args.priority.as_deref(), Some("high"));
    assert_eq!(args.subscriber, ["Alice", "Bob"]);
    assert_eq!(args.output, OutputFormat::Table);

    let update = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "update",
        "abcd",
        "--project=",
        "--clear-subscribers",
    ])
    .expect("autopilot update CLI");
    let Command::Autopilot(AutopilotArgs {
        command: AutopilotCommand::Update(args),
    }) = update.command
    else {
        panic!("expected autopilot update");
    };
    assert_eq!(args.id, "abcd");
    assert_eq!(args.project.as_deref(), Some(""));
    assert!(args.clear_subscribers);
    assert_eq!(args.output, OutputFormat::Json);

    let delete = Cli::try_parse_from(["cordy", "autopilot", "delete", "abcd"])
        .expect("autopilot delete CLI");
    let Command::Autopilot(AutopilotArgs {
        command: AutopilotCommand::Delete { id },
    }) = delete.command
    else {
        panic!("expected autopilot delete");
    };
    assert_eq!(id, "abcd");
}

#[tokio::test]
async fn autopilot_create_resolves_references_and_preserves_go_body() {
    const AGENT_ID: &str = "11111111-1111-1111-1111-111111111111";
    const PROJECT_ID: &str = "22222222-2222-2222-2222-222222222222";
    const USER_ID: &str = "33333333-3333-3333-3333-333333333333";
    let captured = Arc::new(Mutex::new(None));
    let captured_handler = Arc::clone(&captured);
    let app = Router::new()
        .route(
            "/api/agents",
            get(|request: Request| async move {
                assert_eq!(request.uri().query(), Some("workspace_id=workspace-1"));
                Json(vec![
                    serde_json::json!({"id":AGENT_ID,"name":"Daily Planner"}),
                ])
            }),
        )
        .route(
            "/api/projects",
            get(|request: Request| async move {
                assert_eq!(request.uri().query(), Some("workspace_id=workspace-1"));
                Json(serde_json::json!({
                    "projects":[{"id":PROJECT_ID,"title":"Operations","status":"planned"}]
                }))
            }),
        )
        .route(
            "/api/workspaces/workspace-1/members",
            get(|| async {
                Json(vec![serde_json::json!({
                    "user_id":USER_ID,
                    "name":"Alice",
                    "email":"alice@example.com"
                })])
            }),
        )
        .route(
            "/api/autopilots",
            post(move |Json(body): Json<Value>| {
                let captured = Arc::clone(&captured_handler);
                async move {
                    *captured.lock().expect("captured body") = Some(body.clone());
                    Json(serde_json::json!({
                        "id":"autopilot-1",
                        "title":body["title"],
                        "server_only":"preserved"
                    }))
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
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");
    environment.set("CORDY_TOKEN", "token-1");
    let cli = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "create",
        "--title",
        "Daily planner",
        "--description",
        "Plan each day",
        "--agent",
        "planner",
        "--mode",
        "create_issue",
        "--priority",
        "high",
        "--project",
        "2222",
        "--issue-title-template",
        "Daily {{date}}",
        "--subscriber",
        "Alice",
        "--subscriber",
        "alice@example.com",
        "--output",
        "table",
    ])
    .expect("autopilot create CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("create autopilot");
    assert_eq!(
        output.stdout,
        "Autopilot created: Daily planner (autopilot-1)\n"
    );
    let body = captured
        .lock()
        .expect("captured body")
        .clone()
        .expect("request body");
    assert_eq!(body["title"], "Daily planner");
    assert_eq!(body["description"], "Plan each day");
    assert_eq!(body["assignee_id"], AGENT_ID);
    assert_eq!(body["execution_mode"], "create_issue");
    assert_eq!(body["priority"], "high");
    assert_eq!(body["project_id"], PROJECT_ID);
    assert_eq!(body["issue_title_template"], "Daily {{date}}");
    assert_eq!(
        body["subscribers"],
        serde_json::json!([{"user_type":"member","user_id":USER_ID}])
    );
    server.abort();
}

#[tokio::test]
async fn autopilot_create_rejects_missing_and_invalid_required_values() {
    const AGENT_ID: &str = "11111111-1111-1111-1111-111111111111";
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", "http://127.0.0.1:9");
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");

    for (argv, expected) in [
        (vec!["cordy", "autopilot", "create"], "--title is required"),
        (
            vec!["cordy", "autopilot", "create", "--title", "Daily"],
            "--agent is required (agent name or ID)",
        ),
        (
            vec![
                "cordy",
                "autopilot",
                "create",
                "--title",
                "Daily",
                "--agent",
                AGENT_ID,
            ],
            "--mode is required (create_issue or run_only)",
        ),
        (
            vec![
                "cordy",
                "autopilot",
                "create",
                "--title",
                "Daily",
                "--agent",
                AGENT_ID,
                "--mode",
                "invalid",
            ],
            "--mode must be create_issue or run_only",
        ),
    ] {
        let cli = Cli::try_parse_from(argv).expect("autopilot create CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("invalid create rejected");
        assert_eq!(error.to_string(), expected);
    }
}

#[tokio::test]
async fn autopilot_update_resolves_references_and_patches_only_changed_fields() {
    const AUTOPILOT_ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    const AGENT_ID: &str = "11111111-1111-1111-1111-111111111111";
    const PROJECT_ID: &str = "22222222-2222-2222-2222-222222222222";
    const USER_ID: &str = "33333333-3333-3333-3333-333333333333";
    let captured = Arc::new(Mutex::new(None));
    let captured_handler = Arc::clone(&captured);
    let app = Router::new()
        .route(
            "/api/agents",
            get(|| async {
                Json(vec![
                    serde_json::json!({"id":AGENT_ID,"name":"Codex Agent"}),
                ])
            }),
        )
        .route(
            "/api/projects",
            get(|| async {
                Json(serde_json::json!({"projects":[{"id":PROJECT_ID,"title":"Ops"}]}))
            }),
        )
        .route(
            "/api/workspaces/workspace-1/members",
            get(|| async { Json(vec![serde_json::json!({"user_id":USER_ID,"name":"Alice"})]) }),
        )
        .route(
            "/api/autopilots/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            patch(move |Json(body): Json<Value>| {
                let captured = Arc::clone(&captured_handler);
                async move {
                    *captured.lock().expect("captured body") = Some(body);
                    Json(serde_json::json!({"id":AUTOPILOT_ID,"title":"Updated"}))
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
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");
    let cli = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "update",
        AUTOPILOT_ID,
        "--title",
        "Updated",
        "--description=",
        "--agent",
        "Codex",
        "--project",
        "2222",
        "--priority",
        "urgent",
        "--status",
        "paused",
        "--mode",
        "run_only",
        "--issue-title-template=",
        "--subscriber",
        "Alice",
        "--output",
        "table",
    ])
    .expect("autopilot update CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("update autopilot");
    assert_eq!(
        output.stdout,
        format!("Autopilot updated: Updated ({AUTOPILOT_ID})\n")
    );
    let body = captured
        .lock()
        .expect("captured body")
        .clone()
        .expect("request body");
    assert_eq!(body["title"], "Updated");
    assert_eq!(body["description"], "");
    assert_eq!(body["assignee_type"], "agent");
    assert_eq!(body["assignee_id"], AGENT_ID);
    assert_eq!(body["project_id"], PROJECT_ID);
    assert_eq!(body["priority"], "urgent");
    assert_eq!(body["status"], "paused");
    assert_eq!(body["execution_mode"], "run_only");
    assert_eq!(body["issue_title_template"], "");
    assert_eq!(
        body["subscribers"],
        serde_json::json!([{"user_type":"member","user_id":USER_ID}])
    );
    assert_eq!(body.as_object().map(serde_json::Map::len), Some(10));
    server.abort();
}

#[tokio::test]
async fn autopilot_update_preserves_clear_and_no_change_semantics() {
    const ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    let captured = Arc::new(Mutex::new(None));
    let captured_handler = Arc::clone(&captured);
    let app = Router::new().route(
        "/api/autopilots/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
        patch(move |Json(body): Json<Value>| {
            let captured = Arc::clone(&captured_handler);
            async move {
                *captured.lock().expect("captured body") = Some(body);
                Json(serde_json::json!({"id":ID,"title":"Daily"}))
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
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");
    let clear = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "update",
        ID,
        "--project=",
        "--clear-subscribers",
    ])
    .expect("autopilot clear CLI");
    run_with_input(&clear, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("clear autopilot fields");
    let body = captured
        .lock()
        .expect("captured body")
        .clone()
        .expect("request body");
    assert!(body["project_id"].is_null());
    assert_eq!(body["subscribers"], serde_json::json!([]));

    let no_change =
        Cli::try_parse_from(["cordy", "autopilot", "update", ID]).expect("autopilot no-change CLI");
    let error = run_with_input(&no_change, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("no-change update rejected");
    assert_eq!(
        error.to_string(),
        "no fields to update; use flags like --title, --description, --agent, --status, --mode, etc."
    );

    let conflict = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "update",
        ID,
        "--subscriber",
        "Alice",
        "--clear-subscribers",
    ])
    .expect("autopilot subscriber conflict CLI");
    let error = run_with_input(&conflict, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("subscriber conflict rejected");
    assert_eq!(
        error.to_string(),
        "--subscriber and --clear-subscribers are mutually exclusive"
    );
    server.abort();
}

#[tokio::test]
async fn autopilot_delete_resolves_prefix_and_reports_title() {
    const ID: &str = "abcd0000-1111-2222-3333-444444444444";
    let app = Router::new()
        .route(
            "/api/autopilots",
            get(|request: Request| async move {
                assert_eq!(
                    request.uri().query(),
                    Some("limit=50&workspace_id=workspace-1")
                );
                Json(serde_json::json!({
                    "autopilots":[{"id":ID,"title":"Daily planner","status":"active"}],
                    "total":1
                }))
            }),
        )
        .route(
            "/api/autopilots/abcd0000-1111-2222-3333-444444444444",
            delete_route(|| async { axum::http::StatusCode::NO_CONTENT }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");
    let cli = Cli::try_parse_from(["cordy", "autopilot", "delete", "abcd"])
        .expect("autopilot delete CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("delete autopilot");
    assert_eq!(output.stdout, "Autopilot Daily planner deleted.\n");
    server.abort();
}

#[tokio::test]
async fn autopilot_trigger_and_runs_match_go_requests_and_outputs() {
    const ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    let app = Router::new()
        .route(
            "/api/autopilots/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/trigger",
            post(|Json(body): Json<Value>| async move {
                assert!(body.is_null());
                Json(serde_json::json!({
                    "id":"run-1",
                    "status":"queued",
                    "server_only":"preserved"
                }))
            }),
        )
        .route(
            "/api/autopilots/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/runs",
            get(|request: Request| async move {
                assert_eq!(request.uri().query(), Some("limit=5&offset=2"));
                Json(serde_json::json!({
                    "runs":[{
                        "id":"run-1",
                        "source":"manual",
                        "status":"completed",
                        "issue_id":"issue-1",
                        "triggered_at":"2026-08-24T01:00:00Z",
                        "completed_at":"2026-08-24T01:01:00Z",
                        "server_only":"preserved"
                    }],
                    "total":1
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
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");

    let trigger = Cli::try_parse_from(["cordy", "autopilot", "trigger", ID, "--output", "table"])
        .expect("autopilot trigger CLI");
    let output = run_with_input(&trigger, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("trigger autopilot");
    assert_eq!(
        output.stdout,
        "Autopilot triggered: run run-1 (status: queued)\n"
    );

    let runs = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "runs",
        ID,
        "--limit",
        "5",
        "--offset",
        "2",
    ])
    .expect("autopilot runs table CLI");
    let output = run_with_input(&runs, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list autopilot runs");
    assert!(output.stdout.starts_with("ID"));
    assert!(output.stdout.contains("run-1"));
    assert!(output.stdout.contains("manual"));
    assert!(output.stdout.contains("issue-1"));

    let runs_json = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "runs",
        ID,
        "--limit",
        "5",
        "--offset",
        "2",
        "--output",
        "json",
    ])
    .expect("autopilot runs JSON CLI");
    let output = run_with_input(&runs_json, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list autopilot runs JSON");
    let value: Value = serde_json::from_str(&output.stdout).expect("JSON output");
    assert_eq!(value["total"], 1);
    assert_eq!(value["runs"][0]["server_only"], "preserved");
    server.abort();
}

#[tokio::test]
async fn autopilot_trigger_add_preserves_schedule_and_webhook_semantics() {
    const ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    let captured = Arc::new(Mutex::new(Vec::new()));
    let captured_handler = Arc::clone(&captured);
    let app = Router::new().route(
        "/api/autopilots/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/triggers",
        post(move |Json(body): Json<Value>| {
            let captured = Arc::clone(&captured_handler);
            async move {
                captured.lock().expect("captured bodies").push(body.clone());
                if body["kind"] == "webhook" {
                    Json(serde_json::json!({
                        "id":"trigger-webhook",
                        "kind":"webhook",
                        "webhook_url":"https://hooks.example/direct",
                        "webhook_path":"/ignored"
                    }))
                } else {
                    Json(serde_json::json!({"id":"trigger-schedule","kind":"schedule"}))
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
    environment.set("CORDY_SERVER_URL", format!("http://{address}/"));
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");

    let schedule = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "trigger-add",
        ID,
        "--cron",
        "0 9 * * *",
        "--timezone",
        "America/New_York",
        "--label",
        "Morning",
    ])
    .expect("schedule trigger CLI");
    run_with_input(&schedule, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("create schedule trigger");

    let webhook = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "trigger-add",
        ID,
        "--kind",
        "webhook",
        "--label",
        "GitHub",
        "--output",
        "table",
    ])
    .expect("webhook trigger CLI");
    let output = run_with_input(&webhook, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("create webhook trigger");
    assert_eq!(
        output.stdout,
        "Trigger created: trigger-webhook (kind=webhook)\nWebhook URL: https://hooks.example/direct\n"
    );
    let bodies = captured.lock().expect("captured bodies");
    assert_eq!(bodies[0]["kind"], "schedule");
    assert_eq!(bodies[0]["cron_expression"], "0 9 * * *");
    assert_eq!(bodies[0]["timezone"], "America/New_York");
    assert_eq!(bodies[0]["label"], "Morning");
    assert_eq!(
        bodies[1],
        serde_json::json!({"kind":"webhook","label":"GitHub"})
    );
    server.abort();
}

#[tokio::test]
async fn autopilot_trigger_add_rejects_invalid_kind_specific_flags() {
    const ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", "http://127.0.0.1:9");
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");
    for (extra, expected) in [
        (
            vec!["--kind", "invalid"],
            "--kind must be schedule or webhook",
        ),
        (vec![], "--cron is required for --kind schedule"),
        (
            vec!["--kind", "webhook", "--timezone", "UTC"],
            "--timezone is only valid with --kind schedule",
        ),
        (
            vec!["--kind", "webhook", "--cron", "* * * * *"],
            "--cron is only valid with --kind schedule",
        ),
    ] {
        let mut argv = vec!["cordy", "autopilot", "trigger-add", ID];
        argv.extend(extra);
        let cli = Cli::try_parse_from(argv).expect("trigger-add CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("invalid trigger rejected");
        assert_eq!(error.to_string(), expected);
    }
}

#[tokio::test]
async fn autopilot_trigger_rotate_url_matches_go_confirmation_request_and_output() {
    const AUTOPILOT_ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    const TRIGGER_ID: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let calls_handler = Arc::clone(&calls);
    let app = Router::new().route(
        "/api/autopilots/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/triggers/bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb/rotate-webhook-token",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let calls = Arc::clone(&calls_handler);
            async move {
                assert_eq!(headers["authorization"], "Bearer token-1");
                assert_eq!(headers["x-workspace-id"], "workspace-1");
                assert!(body.is_null());
                let call_number = {
                    let mut captured = calls.lock().expect("captured rotate calls");
                    captured.push(body);
                    captured.len()
                };
                Json(serde_json::json!({
                    "id": TRIGGER_ID,
                    "webhook_url": format!("https://hooks.example/new-secret-{call_number}"),
                    "server_only": "preserved"
                }))
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
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");
    environment.set("CORDY_TOKEN", "token-1");

    let declined = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "trigger-rotate-url",
        AUTOPILOT_ID,
        TRIGGER_ID,
    ])
    .expect("rotate confirmation CLI");
    let output = run_with_input(&declined, &environment, &mut Cursor::new(b"n\n".to_vec()))
        .await
        .expect("declined rotate");
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert!(calls.lock().expect("captured rotate calls").is_empty());

    let table = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "trigger-rotate-url",
        AUTOPILOT_ID,
        TRIGGER_ID,
        "--yes",
        "--output",
        "table",
    ])
    .expect("rotate table CLI");
    let output = run_with_input(&table, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("table rotate");
    assert_eq!(
        output.stdout,
        "Webhook URL rotated for trigger bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb\nWebhook URL: https://hooks.example/new-secret-1\n"
    );

    let json = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "trigger-rotate-url",
        AUTOPILOT_ID,
        TRIGGER_ID,
        "--yes",
    ])
    .expect("rotate JSON CLI");
    let output = run_with_input(&json, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("JSON rotate");
    let result: Value = serde_json::from_str(&output.stdout).expect("rotate JSON");
    assert_eq!(result["id"], TRIGGER_ID);
    assert_eq!(result["webhook_url"], "https://hooks.example/new-secret-2");
    assert_eq!(result["server_only"], "preserved");
    assert!(output.stderr.is_empty());
    assert_eq!(calls.lock().expect("captured rotate calls").len(), 2);
    server.abort();
}

#[tokio::test]
async fn autopilot_trigger_update_and_delete_resolve_prefixes_and_mutate() {
    const AUTOPILOT_ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    const TRIGGER_ID: &str = "bbbb0000-1111-2222-3333-444444444444";
    let captured = Arc::new(Mutex::new(None));
    let captured_handler = Arc::clone(&captured);
    let app = Router::new()
        .route(
            "/api/autopilots/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            get(|| async {
                Json(serde_json::json!({
                    "autopilot":{"id":AUTOPILOT_ID},
                    "triggers":[{"id":TRIGGER_ID,"kind":"schedule","label":"Morning"}]
                }))
            }),
        )
        .route(
            "/api/autopilots/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/triggers/bbbb0000-1111-2222-3333-444444444444",
            patch(move |Json(body): Json<Value>| {
                let captured = Arc::clone(&captured_handler);
                async move {
                    *captured.lock().expect("captured body") = Some(body);
                    Json(serde_json::json!({"id":TRIGGER_ID,"enabled":false}))
                }
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

    let update = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "trigger-update",
        AUTOPILOT_ID,
        "bbbb",
        "--enabled=false",
        "--cron=",
        "--timezone",
        "UTC",
        "--label=",
        "--output",
        "table",
    ])
    .expect("trigger-update CLI");
    let output = run_with_input(&update, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("update trigger");
    assert_eq!(output.stdout, format!("Trigger updated: {TRIGGER_ID}\n"));
    assert_eq!(
        captured.lock().expect("captured body").as_ref(),
        Some(&serde_json::json!({
            "enabled":false,
            "cron_expression":"",
            "timezone":"UTC",
            "label":""
        }))
    );

    let delete =
        Cli::try_parse_from(["cordy", "autopilot", "trigger-delete", AUTOPILOT_ID, "bbbb"])
            .expect("trigger-delete CLI");
    let output = run_with_input(&delete, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("delete trigger");
    assert_eq!(output.stdout, format!("Trigger {TRIGGER_ID} deleted.\n"));
    server.abort();
}

#[tokio::test]
async fn autopilot_trigger_update_rejects_no_changes_before_requests() {
    const AUTOPILOT_ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    const TRIGGER_ID: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", "http://127.0.0.1:9");
    let cli = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "trigger-update",
        AUTOPILOT_ID,
        TRIGGER_ID,
    ])
    .expect("trigger-update CLI");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("no-change trigger rejected");
    assert_eq!(
        error.to_string(),
        "no fields to update; use --enabled, --cron, --timezone, or --label"
    );
}

#[tokio::test]
async fn autopilot_list_matches_go_filter_actor_and_output_semantics() {
    let app = Router::new()
        .route(
            "/api/autopilots",
            get(|request: Request| async move {
                assert_eq!(request.uri().query(), Some("status=paused"));
                Json(serde_json::json!({
                    "autopilots":[{
                        "id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                        "title":"Nightly review",
                        "status":"paused",
                        "execution_mode":"run_only",
                        "assignee_id":"agent-1",
                        "last_run_at":"2026-08-24T01:02:03Z",
                        "server_only":"preserved"
                    }],
                    "total":1
                }))
            }),
        )
        .route(
            "/api/agents",
            get(|request: Request| async move {
                assert_eq!(request.uri().query(), Some("workspace_id=workspace-1"));
                Json(vec![serde_json::json!({"id":"agent-1","name":"Reviewer"})])
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
    let cli = Cli::try_parse_from(["cordy", "autopilot", "list", "--status", "paused"])
        .expect("autopilot list CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list autopilots");
    assert!(output.stdout.starts_with("ID"));
    assert!(output.stdout.contains("aaaaaaaa"));
    assert!(!output.stdout.contains("aaaaaaaa-aaaa"));
    assert!(output.stdout.contains("Nightly review"));
    assert!(output.stdout.contains("Reviewer"));
    assert!(output.stdout.contains("2026-08-24T01:02:03Z"));

    let json = Cli::try_parse_from([
        "cordy",
        "autopilot",
        "list",
        "--status",
        "paused",
        "--output",
        "json",
    ])
    .expect("autopilot list JSON CLI");
    let output = run_with_input(&json, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list autopilots as JSON");
    let value: Value = serde_json::from_str(&output.stdout).expect("JSON output");
    assert_eq!(value["total"], 1);
    assert_eq!(value["autopilots"][0]["server_only"], "preserved");
    server.abort();
}

#[tokio::test]
async fn autopilot_get_resolves_prefix_and_preserves_detail_envelope() {
    const ID: &str = "abcd0000-1111-2222-3333-444444444444";
    let app = Router::new()
        .route(
            "/api/autopilots",
            get(|request: Request| async move {
                match request.uri().query() {
                    Some("limit=50&workspace_id=workspace-1") => Json(serde_json::json!({
                        "autopilots":(0..50).map(|index| serde_json::json!({
                            "id":format!("{index:08x}-1111-2222-3333-444444444444")
                        })).collect::<Vec<_>>(),
                        "total":51,
                        "has_more":true
                    })),
                    Some("limit=50&offset=50&workspace_id=workspace-1") => {
                        Json(serde_json::json!({
                            "autopilots":[{"id":ID,"title":"Morning triage","status":"active"}],
                            "total":51,
                            "has_more":false
                        }))
                    }
                    query => panic!("unexpected resolver query: {query:?}"),
                }
            }),
        )
        .route(
            "/api/autopilots/abcd0000-1111-2222-3333-444444444444",
            get(|| async {
                Json(serde_json::json!({
                    "autopilot":{
                        "id":ID,
                        "title":"Morning triage",
                        "status":"active",
                        "execution_mode":"create_issue",
                        "assignee_id":"agent-1",
                        "last_run_at":null
                    },
                    "triggers":[{"id":"trigger-1","kind":"schedule"}],
                    "collaborators":[],
                    "server_only":"preserved"
                }))
            }),
        )
        .route(
            "/api/agents",
            get(|| async { Json(vec![serde_json::json!({"id":"agent-1","name":"Planner"})]) }),
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

    let table = Cli::try_parse_from(["cordy", "autopilot", "get", "abcd", "--output", "table"])
        .expect("autopilot get table CLI");
    let output = run_with_input(&table, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("get autopilot table");
    assert!(output.stdout.contains(ID));
    assert!(output.stdout.contains("Planner"));

    let json =
        Cli::try_parse_from(["cordy", "autopilot", "get", ID]).expect("autopilot get JSON CLI");
    let output = run_with_input(&json, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("get autopilot JSON");
    let value: Value = serde_json::from_str(&output.stdout).expect("JSON output");
    assert_eq!(value["triggers"][0]["kind"], "schedule");
    assert_eq!(value["server_only"], "preserved");
    server.abort();
}

#[tokio::test]
async fn autopilot_prefix_errors_match_go_resolver_contract() {
    let app = Router::new().route(
        "/api/autopilots",
        get(|| async {
            Json(serde_json::json!({
                "autopilots":[
                    {"id":"abcd0000-1111-2222-3333-444444444444"},
                    {"id":"abcd9999-1111-2222-3333-444444444444"}
                ],
                "total":2
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
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");

    let short =
        Cli::try_parse_from(["cordy", "autopilot", "get", "abc"]).expect("short prefix CLI");
    let error = run_with_input(&short, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("short prefix rejected");
    assert_eq!(
        error.to_string(),
        "resolve autopilot: resolve autopilot: expected a full UUID or at least 4 hex characters, got \"abc\""
    );

    let ambiguous =
        Cli::try_parse_from(["cordy", "autopilot", "get", "abcd"]).expect("ambiguous prefix CLI");
    let error = run_with_input(&ambiguous, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("ambiguous prefix rejected");
    assert!(error
        .to_string()
        .starts_with("resolve autopilot: ambiguous autopilot id prefix \"abcd\"; matches:"));
    assert!(error
        .to_string()
        .contains("abcd0000-1111-2222-3333-444444444444"));
    assert!(error
        .to_string()
        .contains("abcd9999-1111-2222-3333-444444444444"));
    server.abort();
}
