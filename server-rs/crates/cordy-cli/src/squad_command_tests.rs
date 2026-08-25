use super::*;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete as delete_route, get, patch, post, put};
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use tokio::net::TcpListener;

#[test]
fn squad_list_parses_output_and_matches_go_table_columns() {
    let cli = Cli::try_parse_from(["cordy", "squad", "list", "--output", "json"])
        .expect("squad list CLI");
    let Command::Squad(SquadArgs {
        command: SquadCommand::List { output },
    }) = cli.command
    else {
        panic!("expected squad list");
    };
    assert_eq!(output, OutputFormat::Json);

    let squads = vec![
        serde_json::json!({
            "id": "squad-1",
            "name": "Reviewers",
            "leader_id": "agent-1",
            "member_count": 3
        }),
        serde_json::json!({
            "id": "squad-2",
            "name": "Empty",
            "leader_id": "agent-2",
            "member_count": 0
        }),
        serde_json::json!({
            "id": "squad-3",
            "name": "Legacy",
            "leader_id": "agent-3"
        }),
    ];
    let table = format_squad_list_table(&squads);
    assert!(table.starts_with("ID"));
    assert!(table.contains("LEADER ID"));
    assert!(table.contains("squad-1"));
    assert!(table.contains("Reviewers"));
    assert!(table.contains("3\n"));
    assert!(table.contains("Empty"));
    assert!(table.contains("Legacy"));
    assert_eq!(squad_member_count_display(&squads[1]), "-");
    assert_eq!(squad_member_count_display(&squads[2]), "-");
}

#[tokio::test]
async fn squad_list_uses_authenticated_workspace_endpoint_and_json_shape() {
    let app = Router::new().route(
        "/api/squads",
        get(|request: Request| async move {
            assert_eq!(request.headers()["authorization"], "Bearer token-1");
            assert_eq!(request.headers()["x-workspace-id"], "workspace-1");
            Json(serde_json::json!([{
                "id": "squad-1",
                "name": "Reviewers",
                "leader_id": "agent-1",
                "member_count": 2,
                "created_at": "2026-08-24T00:00:00Z"
            }]))
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

    let cli = Cli::try_parse_from(["cordy", "squad", "list", "--output", "json"])
        .expect("squad list CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("squad list");
    let squads: Value = serde_json::from_str(&output.stdout).expect("squad JSON");
    assert_eq!(squads[0]["id"], "squad-1");
    assert_eq!(squads[0]["member_count"], 2);
    assert!(!output.stdout.contains("token-1"));
    server.abort();
}

#[test]
fn squad_get_parses_default_table_output_and_preserves_optional_instructions() {
    let cli = Cli::try_parse_from(["cordy", "squad", "get", "squad-1"]).expect("squad get CLI");
    let Command::Squad(SquadArgs {
        command: SquadCommand::Get { squad_id, output },
    }) = cli.command
    else {
        panic!("expected squad get");
    };
    assert_eq!(squad_id, "squad-1");
    assert_eq!(output, OutputFormat::Table);

    let squad = serde_json::json!({
        "id": "squad-1",
        "name": "Reviewers",
        "description": "Review changes",
        "leader_id": "agent-1",
        "created_at": "2026-08-24T00:00:00Z",
        "instructions": "Check tests before approval"
    });
    let table = format_squad_details_table(&squad);
    assert!(table.contains("ID:           squad-1\n"));
    assert!(table.contains("Description:  Review changes\n"));
    assert!(table.contains("Instructions: Check tests before approval\n"));
    assert!(
        !format_squad_details_table(&serde_json::json!({
            "id": "squad-2"
        }))
        .contains("Instructions:")
    );
}

#[tokio::test]
async fn squad_get_uses_encoded_authenticated_endpoint_and_table_contract() {
    let app = Router::new().route(
        "/api/squads/squad-1",
        get(|request: Request| async move {
            assert_eq!(request.headers()["authorization"], "Bearer token-1");
            assert_eq!(request.headers()["x-workspace-id"], "workspace-1");
            Json(serde_json::json!({
                "id": "squad-1",
                "name": "Reviewers",
                "description": "Review changes",
                "leader_id": "agent-1",
                "created_at": "2026-08-24T00:00:00Z",
                "instructions": "Check tests before approval"
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
    environment.set("CORDY_TOKEN", "token-1");

    let cli = Cli::try_parse_from(["cordy", "squad", "get", "squad-1", "--output", "table"])
        .expect("squad get table CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("squad get");
    assert!(output.stdout.contains("Name:         Reviewers\n"));
    assert!(
        output
            .stdout
            .contains("Instructions: Check tests before approval\n")
    );
    assert!(!output.stdout.contains("token-1"));

    let error = run_squad_get(&cli, &environment, " ", OutputFormat::Json)
        .await
        .expect_err("empty squad ID");
    assert_eq!(error.to_string(), "squad ID must not be empty");
    server.abort();
}

#[tokio::test]
async fn squad_create_resolves_leader_and_posts_go_compatible_body() {
    let app = Router::new()
        .route(
            "/api/agents",
            get(|request: Request| async move {
                assert_eq!(request.headers()["authorization"], "Bearer token-1");
                assert_eq!(request.headers()["x-workspace-id"], "workspace-1");
                assert_eq!(request.uri().query(), Some("workspace_id=workspace-1"));
                Json(serde_json::json!([{"id":"agent-1","name":"Lambda"}]))
            }),
        )
        .route(
            "/api/squads",
            post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                assert_eq!(headers["authorization"], "Bearer token-1");
                assert_eq!(headers["x-workspace-id"], "workspace-1");
                assert_eq!(
                    body,
                    serde_json::json!({
                        "name": "Reviewers",
                        "leader_id": "agent-1",
                        "description": "Review changes"
                    })
                );
                Json(serde_json::json!({"id":"squad-1","name":"Reviewers"}))
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
        "squad",
        "create",
        "--name",
        " Reviewers ",
        "--description",
        "Review changes",
        "--leader",
        "Lambda",
        "--output",
        "table",
    ])
    .expect("squad create CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("squad create");
    assert_eq!(output.stdout, "Squad created: Reviewers (squad-1)\n");
    assert!(output.stderr.is_empty());
    assert!(!output.stdout.contains("token-1"));

    let json_cli = Cli::try_parse_from([
        "cordy",
        "squad",
        "create",
        "--name",
        "Reviewers",
        "--leader",
        "agent-1",
        "--output",
        "json",
    ])
    .expect("squad create JSON CLI");
    let json_output = run_with_input(&json_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("squad create JSON");
    let created: Value = serde_json::from_str(&json_output.stdout).expect("created squad JSON");
    assert_eq!(created["id"], "squad-1");
    assert!(json_output.stderr.is_empty());

    let missing_name = SquadCreateArgs {
        name: None,
        description: String::new(),
        leader: Some("agent-1".into()),
        output: OutputFormat::Json,
    };
    let error = run_squad_create(&cli, &environment, &missing_name)
        .await
        .expect_err("missing name");
    assert_eq!(error.to_string(), "--name is required");
    let missing_leader = SquadCreateArgs {
        name: Some("Reviewers".into()),
        description: String::new(),
        leader: Some(" ".into()),
        output: OutputFormat::Json,
    };
    let error = run_squad_create(&cli, &environment, &missing_leader)
        .await
        .expect_err("missing leader");
    assert_eq!(error.to_string(), "--leader is required (name or ID)");
    server.abort();
}

#[tokio::test]
async fn squad_update_sends_only_explicit_fields_and_resolves_leader() {
    let app = Router::new()
        .route(
            "/api/agents",
            get(|request: Request| async move {
                assert_eq!(request.headers()["authorization"], "Bearer token-1");
                assert_eq!(request.headers()["x-workspace-id"], "workspace-1");
                assert_eq!(request.uri().query(), Some("workspace_id=workspace-1"));
                Json(serde_json::json!([{"id":"agent-1","name":"Lambda"}]))
            }),
        )
        .route(
            "/api/squads/squad-1",
            put(|headers: HeaderMap, Json(body): Json<Value>| async move {
                assert_eq!(headers["authorization"], "Bearer token-1");
                assert_eq!(headers["x-workspace-id"], "workspace-1");
                assert_eq!(
                    body,
                    serde_json::json!({
                        "name": "Updated",
                        "description": "",
                        "instructions": "Check tests",
                        "leader_id": "agent-1",
                        "avatar_url": "https://example.test/avatar.png"
                    })
                );
                Json(serde_json::json!({"id":"squad-1","name":"Updated"}))
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
        "squad",
        "update",
        "squad-1",
        "--name",
        "Updated",
        "--description",
        "",
        "--instructions",
        "Check tests",
        "--leader",
        "Lambda",
        "--avatar-url",
        "https://example.test/avatar.png",
        "--output",
        "table",
    ])
    .expect("squad update CLI");
    let Command::Squad(SquadArgs {
        command: SquadCommand::Update(args),
    }) = &cli.command
    else {
        panic!("expected squad update");
    };
    assert_eq!(args.description.as_deref(), Some(""));
    assert_eq!(
        args.avatar_url.as_deref(),
        Some("https://example.test/avatar.png")
    );

    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("squad update");
    assert_eq!(output.stdout, "Squad updated: Updated (squad-1)\n");
    assert!(output.stderr.is_empty());

    let no_fields = SquadUpdateArgs {
        squad_id: "squad-1".into(),
        name: None,
        description: None,
        instructions: None,
        leader: None,
        avatar_url: None,
        output: OutputFormat::Json,
    };
    let error = run_squad_update(&cli, &environment, &no_fields)
        .await
        .expect_err("no fields");
    assert!(error.to_string().contains("no fields to update"));
    server.abort();
}

#[tokio::test]
async fn squad_delete_matches_go_json_and_table_output_contracts() {
    let app = Router::new().route(
        "/api/squads/squad-1",
        delete_route(|headers: HeaderMap| async move {
            assert_eq!(headers["authorization"], "Bearer token-1");
            assert_eq!(headers["x-workspace-id"], "workspace-1");
            axum::http::StatusCode::NO_CONTENT
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

    let table_cli = Cli::try_parse_from(["cordy", "squad", "delete", "squad-1"])
        .expect("squad delete table CLI");
    let Command::Squad(SquadArgs {
        command: SquadCommand::Delete { output, .. },
    }) = &table_cli.command
    else {
        panic!("expected squad delete");
    };
    assert_eq!(*output, OutputFormat::Table);
    let table = run_with_input(&table_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("delete squad table");
    assert!(table.stdout.is_empty());
    assert_eq!(table.stderr, "Squad squad-1 deleted.\n");

    let json_cli = Cli::try_parse_from(["cordy", "squad", "delete", "squad-1", "--output", "json"])
        .expect("squad delete JSON CLI");
    let json = run_with_input(&json_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("delete squad JSON");
    let result: Value = serde_json::from_str(&json.stdout).expect("delete JSON");
    assert_eq!(result, serde_json::json!({"id":"squad-1","deleted":true}));
    assert!(json.stderr.is_empty());

    let error = run_squad_delete(&table_cli, &environment, " ", OutputFormat::Json)
        .await
        .expect_err("empty squad ID");
    assert_eq!(error.to_string(), "squad ID must not be empty");
    server.abort();
}

#[tokio::test]
async fn squad_member_list_matches_go_table_json_and_empty_output() {
    let app = Router::new().route(
        "/api/squads/squad-1/members",
        get(|request: Request| async move {
            assert_eq!(request.headers()["authorization"], "Bearer token-1");
            assert_eq!(request.headers()["x-workspace-id"], "workspace-1");
            Json(serde_json::json!([
                {"member_id":"agent-1","member_type":"agent","role":"lead"},
                {"member_id":"user-1","member_type":"member","role":"member"}
            ]))
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

    let table_cli = Cli::try_parse_from(["cordy", "squad", "member", "list", "squad-1"])
        .expect("squad member list table CLI");
    let Command::Squad(SquadArgs {
        command:
            SquadCommand::Member(SquadMemberArgs {
                command: SquadMemberCommand::List { output, .. },
            }),
    }) = &table_cli.command
    else {
        panic!("expected squad member list");
    };
    assert_eq!(*output, OutputFormat::Table);
    let table = run_with_input(&table_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list squad members table");
    assert!(table.stdout.starts_with("MEMBER ID"));
    assert!(table.stdout.contains("agent-1"));
    assert!(table.stdout.contains("member\n"));
    assert!(table.stderr.is_empty());

    let json_cli = Cli::try_parse_from([
        "cordy", "squad", "member", "list", "squad-1", "--output", "json",
    ])
    .expect("squad member list JSON CLI");
    let json = run_with_input(&json_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list squad members JSON");
    let members: Value = serde_json::from_str(&json.stdout).expect("member JSON");
    assert_eq!(members[0]["member_type"], "agent");
    assert!(json.stderr.is_empty());

    let empty = render_squad_member_output(&[], OutputFormat::Table).expect("empty output");
    assert!(empty.stdout.is_empty());
    assert_eq!(empty.stderr, "No members found.\n");
    let error = run_squad_member_list(&table_cli, &environment, " ", OutputFormat::Json)
        .await
        .expect_err("empty squad ID");
    assert_eq!(error.to_string(), "squad ID must not be empty");
    server.abort();
}

#[tokio::test]
async fn squad_member_add_validates_and_posts_go_compatible_body() {
    let app = Router::new().route(
        "/api/squads/squad-1/members",
        post(|headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["authorization"], "Bearer token-1");
            assert_eq!(headers["x-workspace-id"], "workspace-1");
            match body["member_id"].as_str() {
                Some("user-1") => assert_eq!(
                    body,
                    serde_json::json!({
                        "member_type": "member",
                        "member_id": "user-1",
                        "role": "maintainer"
                    })
                ),
                Some("agent-1") => assert_eq!(
                    body,
                    serde_json::json!({
                        "member_type": "agent",
                        "member_id": "agent-1",
                        "role": "member"
                    })
                ),
                other => panic!("unexpected member id: {other:?}"),
            }
            Json(body)
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

    let defaults = Cli::try_parse_from([
        "cordy",
        "squad",
        "member",
        "add",
        "squad-1",
        "--member-id",
        "agent-1",
    ])
    .expect("squad member add defaults CLI");
    let Command::Squad(SquadArgs {
        command:
            SquadCommand::Member(SquadMemberArgs {
                command:
                    SquadMemberCommand::Add(SquadMemberAddArgs {
                        member_type,
                        role,
                        output,
                        ..
                    }),
            }),
    }) = &defaults.command
    else {
        panic!("expected squad member add");
    };
    assert_eq!(member_type, "agent");
    assert_eq!(role, "member");
    assert_eq!(*output, OutputFormat::Json);

    let table_cli = Cli::try_parse_from([
        "cordy",
        "squad",
        "member",
        "add",
        "squad-1",
        "--member-id",
        "user-1",
        "--type",
        "member",
        "--role",
        "maintainer",
        "--output",
        "table",
    ])
    .expect("squad member add table CLI");
    let table = run_with_input(&table_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("add squad member table");
    assert!(table.stdout.is_empty());
    assert_eq!(table.stderr, "Member user-1 added to squad.\n");

    let json = run_with_input(&defaults, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("add squad member JSON");
    let result: Value = serde_json::from_str(&json.stdout).expect("member JSON");
    assert_eq!(result["member_id"], "agent-1");
    assert!(json.stderr.is_empty());

    let missing = SquadMemberAddArgs {
        squad_id: "squad-1".into(),
        member_id: None,
        member_type: "agent".into(),
        role: "member".into(),
        output: OutputFormat::Json,
    };
    let error = run_squad_member_add(&defaults, &environment, &missing)
        .await
        .expect_err("missing member id");
    assert_eq!(error.to_string(), "--member-id is required");
    let invalid_type = SquadMemberAddArgs {
        member_id: Some("agent-1".into()),
        member_type: "owner".into(),
        ..missing
    };
    let error = run_squad_member_add(&defaults, &environment, &invalid_type)
        .await
        .expect_err("invalid member type");
    assert_eq!(error.to_string(), "--type must be 'agent' or 'member'");
    server.abort();
}

#[tokio::test]
async fn squad_member_set_role_validates_and_patches_go_compatible_body() {
    let app = Router::new().route(
        "/api/squads/squad-1/members/role",
        patch(|headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["authorization"], "Bearer token-1");
            assert_eq!(headers["x-workspace-id"], "workspace-1");
            match body["member_id"].as_str() {
                Some("user-1") => assert_eq!(
                    body,
                    serde_json::json!({
                        "member_type": "member",
                        "member_id": "user-1",
                        "role": "lead"
                    })
                ),
                Some("agent-1") => assert_eq!(
                    body,
                    serde_json::json!({
                        "member_type": "agent",
                        "member_id": "agent-1",
                        "role": "owner"
                    })
                ),
                other => panic!("unexpected member id: {other:?}"),
            }
            Json(body)
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

    let table_cli = Cli::try_parse_from([
        "cordy",
        "squad",
        "member",
        "set-role",
        "squad-1",
        "--member-id",
        "user-1",
        "--member-type",
        "member",
        "--role",
        "lead",
        "--output",
        "table",
    ])
    .expect("squad member set-role table CLI");
    let Command::Squad(SquadArgs {
        command:
            SquadCommand::Member(SquadMemberArgs {
                command: SquadMemberCommand::SetRole(args),
            }),
    }) = &table_cli.command
    else {
        panic!("expected squad member set-role");
    };
    assert_eq!(args.member_type, "member");
    assert_eq!(args.role.as_deref(), Some("lead"));
    assert_eq!(args.output, OutputFormat::Table);
    let table = run_with_input(&table_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("set squad member role table");
    assert!(table.stdout.is_empty());
    assert_eq!(table.stderr, "Member user-1 role updated to lead.\n");

    let defaults = Cli::try_parse_from([
        "cordy",
        "squad",
        "member",
        "set-role",
        "squad-1",
        "--member-id",
        "agent-1",
        "--role",
        "owner",
    ])
    .expect("squad member set-role defaults CLI");
    let Command::Squad(SquadArgs {
        command:
            SquadCommand::Member(SquadMemberArgs {
                command: SquadMemberCommand::SetRole(args),
            }),
    }) = &defaults.command
    else {
        panic!("expected squad member set-role defaults");
    };
    assert_eq!(args.member_type, "agent");
    assert_eq!(args.output, OutputFormat::Json);
    let json = run_with_input(&defaults, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("set squad member role JSON");
    let result: Value = serde_json::from_str(&json.stdout).expect("set role JSON");
    assert_eq!(result["member_id"], "agent-1");
    assert!(json.stderr.is_empty());

    let missing = SquadMemberSetRoleArgs {
        squad_id: "squad-1".into(),
        member_id: None,
        member_type: "agent".into(),
        role: None,
        output: OutputFormat::Json,
    };
    let error = run_squad_member_set_role(&defaults, &environment, &missing)
        .await
        .expect_err("missing member id");
    assert_eq!(error.to_string(), "--member-id is required");
    let missing_role = SquadMemberSetRoleArgs {
        member_id: Some("agent-1".into()),
        ..missing
    };
    let error = run_squad_member_set_role(&defaults, &environment, &missing_role)
        .await
        .expect_err("missing role");
    assert_eq!(error.to_string(), "--role is required");
    let invalid_type = SquadMemberSetRoleArgs {
        member_id: Some("agent-1".into()),
        member_type: "owner".into(),
        role: Some("lead".into()),
        ..missing_role
    };
    let error = run_squad_member_set_role(&defaults, &environment, &invalid_type)
        .await
        .expect_err("invalid member type");
    assert_eq!(
        error.to_string(),
        "--member-type must be 'agent' or 'member'"
    );
    server.abort();
}

#[tokio::test]
async fn squad_member_remove_validates_and_deletes_with_go_compatible_body() {
    let app = Router::new().route(
        "/api/squads/squad-1/members",
        delete_route(|headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["authorization"], "Bearer token-1");
            assert_eq!(headers["x-workspace-id"], "workspace-1");
            match body["member_id"].as_str() {
                Some("user-1") => assert_eq!(
                    body,
                    serde_json::json!({
                        "member_type": "member",
                        "member_id": "user-1"
                    })
                ),
                Some("agent-1") => assert_eq!(
                    body,
                    serde_json::json!({
                        "member_type": "agent",
                        "member_id": "agent-1"
                    })
                ),
                other => panic!("unexpected member id: {other:?}"),
            }
            axum::http::StatusCode::NO_CONTENT
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

    let table_cli = Cli::try_parse_from([
        "cordy",
        "squad",
        "member",
        "remove",
        "squad-1",
        "--member-id",
        "user-1",
        "--type",
        "member",
        "--output",
        "table",
    ])
    .expect("squad member remove table CLI");
    let Command::Squad(SquadArgs {
        command:
            SquadCommand::Member(SquadMemberArgs {
                command: SquadMemberCommand::Remove(args),
            }),
    }) = &table_cli.command
    else {
        panic!("expected squad member remove");
    };
    assert_eq!(args.member_type, "member");
    assert_eq!(args.output, OutputFormat::Table);
    let table = run_with_input(&table_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("remove squad member table");
    assert!(table.stdout.is_empty());
    assert_eq!(table.stderr, "Member user-1 removed from squad.\n");

    let defaults = Cli::try_parse_from([
        "cordy",
        "squad",
        "member",
        "remove",
        "squad-1",
        "--member-id",
        "agent-1",
    ])
    .expect("squad member remove defaults CLI");
    let Command::Squad(SquadArgs {
        command:
            SquadCommand::Member(SquadMemberArgs {
                command: SquadMemberCommand::Remove(args),
            }),
    }) = &defaults.command
    else {
        panic!("expected squad member remove defaults");
    };
    assert_eq!(args.member_type, "agent");
    assert_eq!(args.output, OutputFormat::Json);
    let json = run_with_input(&defaults, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("remove squad member JSON");
    let result: Value = serde_json::from_str(&json.stdout).expect("remove JSON");
    assert_eq!(
        result,
        serde_json::json!({
            "squad_id": "squad-1",
            "member_id": "agent-1",
            "removed": true
        })
    );
    assert!(json.stderr.is_empty());

    let missing = SquadMemberRemoveArgs {
        squad_id: "squad-1".into(),
        member_id: None,
        member_type: "agent".into(),
        output: OutputFormat::Json,
    };
    let error = run_squad_member_remove(&defaults, &environment, &missing)
        .await
        .expect_err("missing member id");
    assert_eq!(error.to_string(), "--member-id is required");
    let invalid_type = SquadMemberRemoveArgs {
        member_id: Some("agent-1".into()),
        member_type: "owner".into(),
        ..missing
    };
    let error = run_squad_member_remove(&defaults, &environment, &invalid_type)
        .await
        .expect_err("invalid member type");
    assert_eq!(error.to_string(), "--type must be 'agent' or 'member'");
    let empty_squad = SquadMemberRemoveArgs {
        squad_id: " ".into(),
        member_id: Some("agent-1".into()),
        ..invalid_type
    };
    let error = run_squad_member_remove(&defaults, &environment, &empty_squad)
        .await
        .expect_err("empty squad id");
    assert_eq!(error.to_string(), "squad ID must not be empty");
    server.abort();
}

#[tokio::test]
async fn squad_activity_resolves_issue_and_posts_outcome() {
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(|headers: HeaderMap| async move {
                assert_eq!(headers["authorization"], "Bearer token-1");
                assert_eq!(headers["x-workspace-id"], "workspace-1");
                Json(serde_json::json!({
                    "id": "issue-uuid-18",
                    "identifier": "CORD-18"
                }))
            }),
        )
        .route(
            "/api/issues/issue-uuid-18/squad-evaluated",
            post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                assert_eq!(headers["authorization"], "Bearer token-1");
                assert_eq!(headers["x-workspace-id"], "workspace-1");
                match body["outcome"].as_str() {
                    Some("action") => assert_eq!(
                        body,
                        serde_json::json!({
                            "outcome": "action",
                            "reason": "delegated"
                        })
                    ),
                    Some("no_action") => assert_eq!(
                        body,
                        serde_json::json!({
                            "outcome": "no_action",
                            "reason": ""
                        })
                    ),
                    other => panic!("unexpected outcome: {other:?}"),
                }
                Json(body)
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

    let table_cli = Cli::try_parse_from([
        "cordy",
        "squad",
        "activity",
        "CORD-18",
        "action",
        "--reason",
        "delegated",
    ])
    .expect("squad activity table CLI");
    let Command::Squad(SquadArgs {
        command: SquadCommand::Activity(args),
    }) = &table_cli.command
    else {
        panic!("expected squad activity");
    };
    assert_eq!(args.reason, "delegated");
    assert_eq!(args.output, OutputFormat::Table);
    let table = run_with_input(&table_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("record squad activity table");
    assert!(table.stdout.is_empty());
    assert_eq!(
        table.stderr,
        "Squad evaluation recorded: action (issue CORD-18)\n"
    );

    let json_cli = Cli::try_parse_from([
        "cordy",
        "squad",
        "activity",
        "CORD-18",
        "no_action",
        "--output",
        "json",
    ])
    .expect("squad activity JSON CLI");
    let Command::Squad(SquadArgs {
        command: SquadCommand::Activity(args),
    }) = &json_cli.command
    else {
        panic!("expected squad activity JSON");
    };
    assert_eq!(args.reason, "");
    assert_eq!(args.output, OutputFormat::Json);
    let json = run_with_input(&json_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("record squad activity JSON");
    let result: Value = serde_json::from_str(&json.stdout).expect("activity JSON");
    assert_eq!(
        result,
        serde_json::json!({
            "outcome": "no_action",
            "reason": ""
        })
    );
    assert_eq!(
        json.stderr,
        "Squad evaluation recorded: no_action (issue CORD-18)\n"
    );

    let invalid = SquadActivityArgs {
        issue_id: "CORD-18".into(),
        outcome: "retry".into(),
        reason: String::new(),
        output: OutputFormat::Table,
    };
    let error = run_squad_activity(&table_cli, &environment, &invalid)
        .await
        .expect_err("invalid outcome");
    assert_eq!(
        error.to_string(),
        "invalid outcome \"retry\"; valid values: action, no_action, failed"
    );
    server.abort();
}
