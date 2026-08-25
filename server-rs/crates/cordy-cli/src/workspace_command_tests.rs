use super::*;
use super::cli_test_helpers::*;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use clap::Parser;
use std::fs;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[tokio::test]
async fn workspace_list_authenticates_without_workspace_scope() {
    let app = Router::new().route(
        "/api/workspaces",
        get(|request: Request| async move {
            assert_eq!(request.headers()["authorization"], "Bearer workspace-token");
            assert!(request.headers().get("x-workspace-id").is_none());
            Json(serde_json::json!([
                {"id":"11111111-1111-1111-1111-111111111111","name":"Alpha","slug":"alpha"},
                {"id":"22222222-2222-2222-2222-222222222222","name":"Beta","slug":"beta"}
            ]))
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
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_TOKEN", "workspace-token");
    environment.set("CORDY_WORKSPACE_ID", "22222222-2222-2222-2222-222222222222");
    let cli = Cli::try_parse_from(["cordy", "workspace", "list", "--output", "json"])
        .expect("workspace list CLI");

    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("workspace list");

    let workspaces: Value = serde_json::from_str(&output.stdout).expect("JSON output");
    assert_eq!(workspaces.as_array().expect("workspace array").len(), 2);
    assert!(output.stderr.is_empty());
    server.abort();
}

#[test]
fn workspace_table_marks_current_and_honors_full_id() {
    let workspaces = vec![
        WorkspaceSummary {
            id: "11111111-1111-1111-1111-111111111111".into(),
            name: "Alpha".into(),
            slug: "alpha".into(),
        },
        WorkspaceSummary {
            id: "22222222-2222-2222-2222-222222222222".into(),
            name: "Beta".into(),
            slug: "beta".into(),
        },
    ];
    assert_eq!(
        format_workspace_table(&workspaces, "22222222-2222-2222-2222-222222222222", false),
        "   ID        NAME   SLUG\n   11111111  Alpha  alpha\n*  22222222  Beta   beta\n"
    );
    let full = format_workspace_table(&workspaces, "", true);
    assert!(full.contains("11111111-1111-1111-1111-111111111111"));
    assert!(!full.contains("*  "));
}

#[tokio::test]
async fn workspace_list_empty_and_missing_auth_match_go_messages() {
    let app = Router::new().route(
        "/api/workspaces",
        get(|| async { Json(serde_json::json!([])) }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_TOKEN", "workspace-token");
    let cli = Cli::try_parse_from(["cordy", "workspace", "list"]).expect("workspace list CLI");

    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("empty workspace list");
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, "No workspaces found.\n");

    environment.set("CORDY_TOKEN", "");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("missing token");
    assert!(
        error
            .to_string()
            .contains("not authenticated: run 'cordy login' first")
    );
    server.abort();
}

#[tokio::test]
async fn workspace_get_resolves_slug_but_bypasses_list_for_full_uuid() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let list_calls = Arc::new(AtomicUsize::new(0));
    let list_calls_by_handler = Arc::clone(&list_calls);
    let workspace_id = "22222222-2222-2222-2222-222222222222";
    let app = Router::new()
        .route(
            "/api/workspaces",
            get(move || {
                let list_calls = Arc::clone(&list_calls_by_handler);
                async move {
                    list_calls.fetch_add(1, Ordering::SeqCst);
                    Json(serde_json::json!([
                        {"id":"11111111-1111-1111-1111-111111111111","name":"Alpha","slug":"alpha"},
                        {"id":"22222222-2222-2222-2222-222222222222","name":"Beta","slug":"beta"}
                    ]))
                }
            }),
        )
        .route(
            "/api/workspaces/22222222-2222-2222-2222-222222222222",
            get(|| async {
                Json(serde_json::json!({
                    "id":"22222222-2222-2222-2222-222222222222",
                    "name":"Beta",
                    "slug":"beta",
                    "description":"Delivery workspace",
                    "context":"Product context"
                }))
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
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_TOKEN", "workspace-token");

    for target in ["BETA", workspace_id] {
        let cli = Cli::try_parse_from(["cordy", "workspace", "get", target, "--output", "json"])
            .expect("workspace get CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("workspace get");
        let workspace: Value = serde_json::from_str(&output.stdout).expect("JSON output");
        assert_eq!(workspace["id"], workspace_id);
    }
    assert_eq!(list_calls.load(Ordering::SeqCst), 1);
    server.abort();
}

#[test]
fn workspace_reference_reports_ambiguous_and_missing_targets() {
    let workspaces = vec![
        WorkspaceSummary {
            id: "abcd1111-1111-1111-1111-111111111111".into(),
            name: "Alpha".into(),
            slug: "alpha".into(),
        },
        WorkspaceSummary {
            id: "abcd2222-2222-2222-2222-222222222222".into(),
            name: "Beta".into(),
            slug: "beta".into(),
        },
    ];
    let ambiguous = resolve_workspace_reference(&workspaces, "abcd")
        .expect_err("ambiguous prefix")
        .to_string();
    assert!(ambiguous.contains("ambiguous workspace id prefix \"abcd\""));
    assert!(ambiguous.contains("Alpha (alpha)"));
    assert!(ambiguous.contains("Beta (beta)"));
    assert!(
        resolve_workspace_reference(&workspaces, "gamma")
            .expect_err("missing slug")
            .to_string()
            .contains("run 'cordy workspace list'")
    );
    assert_eq!(
        resolve_workspace_reference(&workspaces, "ALPHA")
            .expect("case-insensitive slug")
            .id,
        workspaces[0].id
    );
}

#[test]
fn workspace_details_table_truncates_description_and_context_at_sixty_chars() {
    let long = "界".repeat(61);
    let workspace = serde_json::json!({
        "id":"workspace-1",
        "name":"Alpha",
        "slug":"alpha",
        "description":long,
        "context":"x".repeat(60)
    });
    let table = format_workspace_details_table(&workspace);
    assert!(table.contains(&("界".repeat(57) + "...")));
    assert!(table.contains(&"x".repeat(60)));
    assert!(!table.contains(&"界".repeat(58)));
}

#[tokio::test]
async fn workspace_get_without_argument_requires_default_workspace() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let cli = Cli::try_parse_from(["cordy", "workspace", "get"]).expect("workspace get CLI");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("missing default workspace");
    assert!(error.to_string().contains(
        "workspace ID is required: pass an id/slug/prefix as argument or set CORDY_WORKSPACE_ID"
    ));
}

#[tokio::test]
async fn workspace_create_posts_complete_body_without_workspace_scope() {
    let captured = Arc::new(Mutex::new(None));
    let captured_by_handler = Arc::clone(&captured);
    let app = Router::new().route(
        "/api/workspaces",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let captured = Arc::clone(&captured_by_handler);
            async move {
                assert_eq!(headers["authorization"], "Bearer workspace-token");
                assert!(headers.get("x-workspace-id").is_none());
                *captured.lock().expect("capture body") = Some(body.clone());
                Json(serde_json::json!({
                    "id":"33333333-3333-3333-3333-333333333333",
                    "name":body["name"],
                    "slug":body["slug"],
                    "description":body["description"],
                    "context":body["context"]
                }))
            }
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
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_TOKEN", "workspace-token");
    environment.set("CORDY_WORKSPACE_ID", "must-not-be-sent");
    let cli = Cli::try_parse_from([
        "cordy",
        "workspace",
        "create",
        "--name",
        "Support Team",
        "--slug",
        "support-team",
        "--description",
        r"First line\nSecond line",
        "--context-stdin",
        "--issue-prefix",
        "SUP",
        "--output",
        "table",
    ])
    .expect("workspace create CLI");
    let output = run_with_input(
        &cli,
        &environment,
        &mut Cursor::new(b"Customer support context\n".to_vec()),
    )
    .await
    .expect("create workspace");

    let body = captured
        .lock()
        .expect("captured body")
        .clone()
        .expect("request body");
    assert_eq!(body["name"], "Support Team");
    assert_eq!(body["slug"], "support-team");
    assert_eq!(body["description"], "First line\nSecond line");
    assert_eq!(body["context"], "Customer support context");
    assert_eq!(body["issue_prefix"], "SUP");
    assert!(output.stdout.starts_with("ID"));
    assert!(output.stdout.contains("support-team"));
    server.abort();
}

#[test]
fn workspace_create_validates_required_and_safe_input_flags() {
    let missing_name =
        Cli::try_parse_from(["cordy", "workspace", "create", "--slug", "support-team"])
            .expect("missing name CLI");
    assert_eq!(
        build_workspace_create_body(
            create_workspace_args(&missing_name),
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect_err("missing name")
        .to_string(),
        "--name is required"
    );

    let dual_stdin = Cli::try_parse_from([
        "cordy",
        "workspace",
        "create",
        "--name",
        "Support",
        "--slug",
        "support",
        "--description-stdin",
        "--context-stdin",
    ])
    .expect("dual stdin CLI");
    assert!(
        build_workspace_create_body(
            create_workspace_args(&dual_stdin),
            &mut Cursor::new(b"ambiguous".to_vec())
        )
        .expect_err("dual stdin")
        .to_string()
        .contains("a single stdin cannot feed both fields")
    );

    let empty_prefix = Cli::try_parse_from([
        "cordy",
        "workspace",
        "create",
        "--name",
        "Support",
        "--slug",
        "support",
        "--issue-prefix",
        "   ",
    ])
    .expect("empty prefix CLI");
    assert!(
        build_workspace_create_body(
            create_workspace_args(&empty_prefix),
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect_err("empty issue prefix")
        .to_string()
        .contains("omit it to use the server-generated prefix")
    );
}

#[tokio::test]
async fn workspace_update_resolves_slug_and_patches_without_switching_default() {
    let captured = Arc::new(Mutex::new(None));
    let captured_by_handler = Arc::clone(&captured);
    let workspace_id = "44444444-4444-4444-4444-444444444444";
    let app = Router::new()
        .route(
            "/api/workspaces",
            get(|| async {
                Json(serde_json::json!([{
                    "id":"44444444-4444-4444-4444-444444444444",
                    "name":"Before",
                    "slug":"delivery"
                }]))
            }),
        )
        .route(
            "/api/workspaces/44444444-4444-4444-4444-444444444444",
            patch(move |headers: HeaderMap, Json(body): Json<Value>| {
                let captured = Arc::clone(&captured_by_handler);
                async move {
                    assert_eq!(headers["x-workspace-id"], "original-default");
                    *captured.lock().expect("capture body") = Some(body.clone());
                    Json(serde_json::json!({
                        "id":"44444444-4444-4444-4444-444444444444",
                        "name":body["name"],
                        "slug":"delivery",
                        "description":body["description"],
                        "context":"Existing context"
                    }))
                }
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let config_dir = home.path().join(".cordy");
    fs::create_dir_all(&config_dir).expect("config dir");
    fs::write(
        config_dir.join("config.json"),
        format!(
            r#"{{"server_url":"http://{address}","token":"workspace-token","workspace_id":"original-default"}}"#
        ),
    )
    .expect("config");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let cli = Cli::try_parse_from([
        "cordy",
        "workspace",
        "update",
        "delivery",
        "--name",
        "After",
        "--description",
        "",
        "--output",
        "json",
    ])
    .expect("workspace update CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("update workspace");

    let body = captured
        .lock()
        .expect("captured body")
        .clone()
        .expect("request body");
    assert_eq!(body["name"], "After");
    assert_eq!(body["description"], "");
    assert_eq!(
        serde_json::from_str::<Value>(&output.stdout).expect("JSON")["id"],
        workspace_id
    );
    assert_eq!(
        environment
            .load_config("")
            .expect("config after update")
            .workspace_id,
        "original-default"
    );
    server.abort();
}

#[tokio::test]
async fn workspace_update_rejects_no_changes_before_api_setup() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let cli = Cli::try_parse_from([
        "cordy",
        "workspace",
        "update",
        "55555555-5555-5555-5555-555555555555",
    ])
    .expect("empty workspace update CLI");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("no changes");
    assert_eq!(
        error.to_string(),
        "no fields to update; use --name, --description, --context, or --issue-prefix"
    );
}

#[test]
fn workspace_update_supports_safe_files_and_rejects_ambiguous_or_empty_changes() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    fs::write(cwd.path().join("context.md"), "First\nSecond \\n literal\n").expect("context file");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let file_cli = Cli::try_parse_from([
        "cordy",
        "workspace",
        "update",
        "workspace-id",
        "--context-file",
        "context.md",
    ])
    .expect("file CLI");
    let body = build_workspace_update_body(
        update_workspace_args(&file_cli),
        &environment,
        &mut Cursor::new(Vec::<u8>::new()),
    )
    .expect("file body");
    assert_eq!(body["context"], "First\nSecond \\n literal");

    let ambiguous = Cli::try_parse_from([
        "cordy",
        "workspace",
        "update",
        "workspace-id",
        "--description",
        "inline",
        "--description-file",
        "context.md",
    ])
    .expect("ambiguous CLI");
    assert!(
        build_workspace_update_body(
            update_workspace_args(&ambiguous),
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect_err("ambiguous description")
        .to_string()
        .contains("mutually exclusive")
    );

    let empty =
        Cli::try_parse_from(["cordy", "workspace", "update", "workspace-id"]).expect("empty CLI");
    assert!(
        build_workspace_update_body(
            update_workspace_args(&empty),
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect("empty body")
        .is_empty()
    );

    let empty_prefix = Cli::try_parse_from([
        "cordy",
        "workspace",
        "update",
        "workspace-id",
        "--issue-prefix",
        " ",
    ])
    .expect("empty prefix CLI");
    assert!(
        build_workspace_update_body(
            update_workspace_args(&empty_prefix),
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect_err("empty issue prefix")
        .to_string()
        .contains("clearing the prefix is not supported")
    );
}

#[test]
fn workspace_member_parser_and_role_validation_match_go_contract() {
    let cli = Cli::try_parse_from([
        "cordy",
        "workspace",
        "member",
        "invite",
        "ADA@EXAMPLE.COM",
        "alpha",
        "--role",
        "ADMIN",
        "--output",
        "json",
    ])
    .expect("workspace member invite CLI");
    let Command::Workspace(WorkspaceArgs {
        command:
            WorkspaceCommand::Member(WorkspaceMemberArgs {
                command: WorkspaceMemberCommand::Invite(args),
            }),
    }) = &cli.command
    else {
        panic!("expected workspace member invite");
    };
    assert_eq!(args.workspace.as_deref(), Some("alpha"));
    assert_eq!(
        normalize_workspace_invite_role(&args.role).expect("admin"),
        "admin"
    );
    assert!(
        normalize_workspace_invite_role("owner")
            .expect_err("owner rejected")
            .to_string()
            .contains("cannot invite as owner")
    );
    assert!(
        normalize_workspace_invite_role("viewer")
            .expect_err("unknown role")
            .to_string()
            .contains("expected member or admin")
    );
}

#[tokio::test]
async fn workspace_member_list_and_invite_use_go_http_and_output_contracts() {
    let workspace_id = "55555555-5555-5555-5555-555555555555";
    let app = Router::new().route(
        "/api/workspaces/55555555-5555-5555-5555-555555555555/members",
        get(|| async {
            Json(vec![serde_json::json!({
                "user_id":"user-1","name":"Ada","email":"ada@example.com","role":"admin"
            })])
        })
        .post(|Json(body): Json<Value>| async move {
            assert_eq!(
                body,
                serde_json::json!({
                    "email":"new@example.com","role":"member"
                })
            );
            Json(serde_json::json!({
                "invitee_email":"new@example.com","role":"member","status":"pending"
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
    environment.set("CORDY_WORKSPACE_ID", workspace_id);
    environment.set("CORDY_TOKEN", "token-1");

    let list = Cli::try_parse_from([
        "cordy",
        "workspace",
        "member",
        "list",
        workspace_id,
        "--output",
        "table",
    ])
    .expect("workspace member list CLI");
    let listed = run_with_input(&list, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list workspace members");
    assert!(listed.stdout.starts_with("USER ID"));
    assert!(listed.stdout.contains("ada@example.com"));
    assert!(listed.stdout.contains("admin"));

    let invite = Cli::try_parse_from([
        "cordy",
        "workspace",
        "member",
        "invite",
        " NEW@EXAMPLE.COM ",
        workspace_id,
    ])
    .expect("workspace member invite CLI");
    let invited = run_with_input(&invite, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("invite workspace member");
    assert_eq!(
        invited.stdout,
        "Invitation sent to new@example.com (role: member, status: pending)\n"
    );
    server.abort();
}

#[tokio::test]
async fn workspace_switch_verifies_access_and_atomically_updates_only_current_profile() {
    let workspace_id = "55555555-5555-5555-5555-555555555555";
    let app = Router::new().route(
        "/api/workspaces",
        get(move || async move {
            Json(vec![serde_json::json!({
                "id":workspace_id,"name":"Alpha","slug":"alpha"
            })])
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let profile_dir = home.path().join(".cordy/profiles/dev");
    fs::create_dir_all(&profile_dir).expect("profile dir");
    fs::write(
        profile_dir.join("config.json"),
        r#"{"server_url":"old","unknown":{"keep":true}}"#,
    )
    .expect("profile config");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_TOKEN", "token-1");
    let cli = Cli::try_parse_from(["cordy", "--profile", "dev", "workspace", "switch", "ALPHA"])
        .expect("workspace switch CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("switch workspace");
    assert_eq!(
        output.stdout,
        format!("Switched to workspace: Alpha ({workspace_id})\n")
    );
    let document: Value = serde_json::from_slice(
        &fs::read(profile_dir.join("config.json")).expect("updated profile config"),
    )
    .expect("profile JSON");
    assert_eq!(document["workspace_id"], workspace_id);
    assert_eq!(document["unknown"]["keep"], true);
    assert!(!home.path().join(".cordy/config.json").exists());
    server.abort();
}

#[tokio::test]
async fn workspace_mcp_list_drops_secret_config_in_every_output_format() {
    let workspace_id = "55555555-5555-5555-5555-555555555555";
    let app = Router::new().route(
        "/api/workspaces/55555555-5555-5555-5555-555555555555/mcp-servers",
        get(|| async {
            Json(vec![serde_json::json!({
                "id":"server-1","name":"linear","transport":"http",
                "url":"https://secret.example/token","headers":{"Authorization":"Bearer secret"},
                "config":{"env":{"API_KEY":"secret"}}
            })])
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_WORKSPACE_ID", workspace_id);
    environment.set("CORDY_TOKEN", "token-1");

    for output in ["json", "table"] {
        let cli = Cli::try_parse_from([
            "cordy",
            "workspace",
            "mcp",
            "list",
            workspace_id,
            "--output",
            output,
        ])
        .expect("workspace mcp list CLI");
        let listed = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list workspace MCP servers");
        assert!(listed.stdout.contains("linear"));
        assert!(listed.stdout.contains("http"));
        assert!(!listed.stdout.contains("secret"));
        assert!(!listed.stdout.contains("Authorization"));
        assert!(!listed.stdout.contains("API_KEY"));
    }
    server.abort();
}

#[test]
fn workspace_mcp_config_validation_is_secret_safe_and_rejects_non_objects() {
    let secret = r#"{"token":"sk-do-not-echo""#;
    let error = parse_workspace_mcp_server_config(secret).expect_err("invalid JSON");
    assert_eq!(
        error.to_string(),
        "--server-config must be a valid JSON object"
    );
    assert!(!error.to_string().contains("sk-do-not-echo"));
    assert_eq!(
        parse_workspace_mcp_server_config("null")
            .expect_err("null")
            .to_string(),
        "--server-config must be a JSON object, not null"
    );
    assert!(
        parse_workspace_mcp_server_config("[]")
            .expect_err("array")
            .to_string()
            .contains("must be a JSON object")
    );
}

#[tokio::test]
async fn workspace_mcp_mutations_use_safe_inputs_and_never_echo_config() {
    let workspace_id = "55555555-5555-5555-5555-555555555555";
    let endpoint = "/api/workspaces/55555555-5555-5555-5555-555555555555/mcp-servers";
    let resource = "/api/workspaces/55555555-5555-5555-5555-555555555555/mcp-servers/server-1";
    let app = Router::new()
        .route(
            endpoint,
            post(|Json(body): Json<Value>| async move {
                assert_eq!(body["name"], "linear");
                assert_eq!(body["config"]["url"], "https://linear.example");
                Json(serde_json::json!({
                    "id":"server-1","name":"linear","transport":"http",
                    "config":{"url":"https://secret.example","headers":{"Authorization":"secret"}}
                }))
            }),
        )
        .route(
            resource,
            put(|Json(body): Json<Value>| async move {
                assert_eq!(body["name"], "linear-v2");
                assert!(body.get("config").is_none());
                Json(serde_json::json!({
                    "id":"server-1","name":"linear-v2","transport":"stdio",
                    "url":"https://secret.example"
                }))
            })
            .delete(|| async { axum::http::StatusCode::NO_CONTENT }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    fs::write(
        cwd.path().join("linear.json"),
        r#"{"url":"https://linear.example"}"#,
    )
    .expect("MCP config file");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_WORKSPACE_ID", workspace_id);
    environment.set("CORDY_TOKEN", "token-1");

    let add = Cli::try_parse_from([
        "cordy",
        "workspace",
        "mcp",
        "add",
        "linear",
        workspace_id,
        "--server-config-file",
        "linear.json",
    ])
    .expect("workspace MCP add CLI");
    let added = run_with_input(&add, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("add workspace MCP server");
    assert!(added.stdout.contains("linear"));
    assert!(!added.stdout.contains("secret"));
    assert!(!added.stdout.contains("config"));

    let update = Cli::try_parse_from([
        "cordy",
        "workspace",
        "mcp",
        "update",
        "server-1",
        workspace_id,
        "--name",
        " linear-v2 ",
        "--output",
        "table",
    ])
    .expect("workspace MCP update CLI");
    let updated = run_with_input(&update, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("update workspace MCP server");
    assert!(updated.stdout.contains("linear-v2"));
    assert!(!updated.stdout.contains("secret"));

    let remove = Cli::try_parse_from([
        "cordy",
        "workspace",
        "mcp",
        "remove",
        "server-1",
        workspace_id,
    ])
    .expect("workspace MCP remove CLI");
    let removed = run_with_input(&remove, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("remove workspace MCP server");
    assert_eq!(removed.stdout, "removed MCP server server-1\n");
    server.abort();
}
