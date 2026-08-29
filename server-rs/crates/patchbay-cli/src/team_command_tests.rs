use super::*;
use axum::extract::Request;
use axum::http::HeaderMap;
use axum::routing::{delete as delete_route, get, patch, post, put};
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use tokio::net::TcpListener;

#[test]
fn team_list_parses_output_and_matches_go_table_columns() {
    let cli = Cli::try_parse_from(["patchbay", "team", "list", "--output", "json"])
        .expect("team list CLI");
    let Command::Team(TeamArgs {
        command: TeamCommand::List { output },
    }) = cli.command
    else {
        panic!("expected team list");
    };
    assert_eq!(output, OutputFormat::Json);

    let teams = vec![
        serde_json::json!({
            "id": "team-1",
            "name": "Reviewers",
            "leader_id": "agent-1",
            "member_count": 3
        }),
        serde_json::json!({
            "id": "team-2",
            "name": "Empty",
            "leader_id": "agent-2",
            "member_count": 0
        }),
        serde_json::json!({
            "id": "team-3",
            "name": "Legacy",
            "leader_id": "agent-3"
        }),
    ];
    let table = format_team_list_table(&teams);
    assert!(table.starts_with("ID"));
    assert!(table.contains("LEADER ID"));
    assert!(table.contains("team-1"));
    assert!(table.contains("Reviewers"));
    assert!(table.contains("3\n"));
    assert!(table.contains("Empty"));
    assert!(table.contains("Legacy"));
    assert_eq!(team_member_count_display(&teams[1]), "-");
    assert_eq!(team_member_count_display(&teams[2]), "-");
}

#[tokio::test]
async fn team_list_uses_authenticated_workspace_endpoint_and_json_shape() {
    let app = Router::new().route(
        "/api/teams",
        get(|request: Request| async move {
            assert_eq!(request.headers()["authorization"], "Bearer token-1");
            assert_eq!(request.headers()["x-workspace-id"], "workspace-1");
            Json(serde_json::json!([{
                "id": "team-1",
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
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");

    let cli = Cli::try_parse_from(["patchbay", "team", "list", "--output", "json"])
        .expect("team list CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("team list");
    let teams: Value = serde_json::from_str(&output.stdout).expect("team JSON");
    assert_eq!(teams[0]["id"], "team-1");
    assert_eq!(teams[0]["member_count"], 2);
    assert!(!output.stdout.contains("token-1"));
    server.abort();
}

#[test]
fn team_get_parses_default_table_output_and_preserves_optional_instructions() {
    let cli = Cli::try_parse_from(["patchbay", "team", "get", "team-1"]).expect("team get CLI");
    let Command::Team(TeamArgs {
        command: TeamCommand::Get { team_id, output },
    }) = cli.command
    else {
        panic!("expected team get");
    };
    assert_eq!(team_id, "team-1");
    assert_eq!(output, OutputFormat::Table);

    let team = serde_json::json!({
        "id": "team-1",
        "name": "Reviewers",
        "description": "Review changes",
        "leader_id": "agent-1",
        "created_at": "2026-08-24T00:00:00Z",
        "instructions": "Check tests before approval"
    });
    let table = format_team_details_table(&team);
    assert!(table.contains("ID:           team-1\n"));
    assert!(table.contains("Description:  Review changes\n"));
    assert!(table.contains("Instructions: Check tests before approval\n"));
    assert!(!format_team_details_table(&serde_json::json!({
        "id": "team-2"
    }))
    .contains("Instructions:"));
}

#[tokio::test]
async fn team_get_uses_encoded_authenticated_endpoint_and_table_contract() {
    let app = Router::new().route(
        "/api/teams/team-1",
        get(|request: Request| async move {
            assert_eq!(request.headers()["authorization"], "Bearer token-1");
            assert_eq!(request.headers()["x-workspace-id"], "workspace-1");
            Json(serde_json::json!({
                "id": "team-1",
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
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");

    let cli = Cli::try_parse_from(["patchbay", "team", "get", "team-1", "--output", "table"])
        .expect("team get table CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("team get");
    assert!(output.stdout.contains("Name:         Reviewers\n"));
    assert!(output
        .stdout
        .contains("Instructions: Check tests before approval\n"));
    assert!(!output.stdout.contains("token-1"));

    let error = run_team_get(&cli, &environment, " ", OutputFormat::Json)
        .await
        .expect_err("empty team ID");
    assert_eq!(error.to_string(), "team ID must not be empty");
    server.abort();
}

#[tokio::test]
async fn team_create_resolves_leader_and_posts_go_compatible_body() {
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
            "/api/teams",
            post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                assert_eq!(headers["authorization"], "Bearer token-1");
                assert_eq!(headers["x-workspace-id"], "workspace-1");
                assert!(
                    body == serde_json::json!({
                        "name": "Reviewers",
                        "leader_id": "agent-1",
                        "description": "Review changes"
                    }) || body
                        == serde_json::json!({
                            "name": "Reviewers",
                            "leader_id": "agent-1"
                        })
                );
                Json(serde_json::json!({"id":"team-1","name":"Reviewers"}))
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

    let cli = Cli::try_parse_from([
        "patchbay",
        "team",
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
    .expect("team create CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("team create");
    assert_eq!(output.stdout, "Team created: Reviewers (team-1)\n");
    assert!(output.stderr.is_empty());
    assert!(!output.stdout.contains("token-1"));

    let json_cli = Cli::try_parse_from([
        "patchbay",
        "team",
        "create",
        "--name",
        "Reviewers",
        "--leader",
        "agent-1",
        "--output",
        "json",
    ])
    .expect("team create JSON CLI");
    let json_output = run_with_input(&json_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("team create JSON");
    let created: Value = serde_json::from_str(&json_output.stdout).expect("created team JSON");
    assert_eq!(created["id"], "team-1");
    assert!(json_output.stderr.is_empty());

    let missing_name = TeamCreateArgs {
        name: None,
        description: String::new(),
        leader: Some("agent-1".into()),
        output: OutputFormat::Json,
    };
    let error = run_team_create(&cli, &environment, &missing_name)
        .await
        .expect_err("missing name");
    assert_eq!(error.to_string(), "--name is required");
    let missing_leader = TeamCreateArgs {
        name: Some("Reviewers".into()),
        description: String::new(),
        leader: Some(" ".into()),
        output: OutputFormat::Json,
    };
    let error = run_team_create(&cli, &environment, &missing_leader)
        .await
        .expect_err("missing leader");
    assert_eq!(error.to_string(), "--leader is required (name or ID)");
    server.abort();
}

#[tokio::test]
async fn team_update_sends_only_explicit_fields_and_resolves_leader() {
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
            "/api/teams/team-1",
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
                Json(serde_json::json!({"id":"team-1","name":"Updated"}))
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

    let cli = Cli::try_parse_from([
        "patchbay",
        "team",
        "update",
        "team-1",
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
    .expect("team update CLI");
    let Command::Team(TeamArgs {
        command: TeamCommand::Update(args),
    }) = &cli.command
    else {
        panic!("expected team update");
    };
    assert_eq!(args.description.as_deref(), Some(""));
    assert_eq!(
        args.avatar_url.as_deref(),
        Some("https://example.test/avatar.png")
    );

    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("team update");
    assert_eq!(output.stdout, "Team updated: Updated (team-1)\n");
    assert!(output.stderr.is_empty());

    let no_fields = TeamUpdateArgs {
        team_id: "team-1".into(),
        name: None,
        description: None,
        instructions: None,
        leader: None,
        avatar_url: None,
        output: OutputFormat::Json,
    };
    let error = run_team_update(&cli, &environment, &no_fields)
        .await
        .expect_err("no fields");
    assert!(error.to_string().contains("no fields to update"));
    server.abort();
}

#[tokio::test]
async fn team_delete_matches_go_json_and_table_output_contracts() {
    let app = Router::new().route(
        "/api/teams/team-1",
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
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");

    let table_cli = Cli::try_parse_from(["patchbay", "team", "delete", "team-1"])
        .expect("team delete table CLI");
    let Command::Team(TeamArgs {
        command: TeamCommand::Delete { output, .. },
    }) = &table_cli.command
    else {
        panic!("expected team delete");
    };
    assert_eq!(*output, OutputFormat::Table);
    let table = run_with_input(&table_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("delete team table");
    assert!(table.stdout.is_empty());
    assert_eq!(table.stderr, "Team team-1 deleted.\n");

    let json_cli =
        Cli::try_parse_from(["patchbay", "team", "delete", "team-1", "--output", "json"])
            .expect("team delete JSON CLI");
    let json = run_with_input(&json_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("delete team JSON");
    let result: Value = serde_json::from_str(&json.stdout).expect("delete JSON");
    assert_eq!(result, serde_json::json!({"id":"team-1","deleted":true}));
    assert!(json.stderr.is_empty());

    let error = run_team_delete(&table_cli, &environment, " ", OutputFormat::Json)
        .await
        .expect_err("empty team ID");
    assert_eq!(error.to_string(), "team ID must not be empty");
    server.abort();
}

#[tokio::test]
async fn team_member_list_matches_go_table_json_and_empty_output() {
    let app = Router::new().route(
        "/api/teams/team-1/members",
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
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");

    let table_cli = Cli::try_parse_from(["patchbay", "team", "member", "list", "team-1"])
        .expect("team member list table CLI");
    let Command::Team(TeamArgs {
        command:
            TeamCommand::Member(TeamMemberArgs {
                command: TeamMemberCommand::List { output, .. },
            }),
    }) = &table_cli.command
    else {
        panic!("expected team member list");
    };
    assert_eq!(*output, OutputFormat::Table);
    let table = run_with_input(&table_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list team members table");
    assert!(table.stdout.starts_with("MEMBER ID"));
    assert!(table.stdout.contains("agent-1"));
    assert!(table.stdout.contains("member\n"));
    assert!(table.stderr.is_empty());

    let json_cli = Cli::try_parse_from([
        "patchbay", "team", "member", "list", "team-1", "--output", "json",
    ])
    .expect("team member list JSON CLI");
    let json = run_with_input(&json_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list team members JSON");
    let members: Value = serde_json::from_str(&json.stdout).expect("member JSON");
    assert_eq!(members[0]["member_type"], "agent");
    assert!(json.stderr.is_empty());

    let empty = render_team_member_output(&[], OutputFormat::Table).expect("empty output");
    assert!(empty.stdout.is_empty());
    assert_eq!(empty.stderr, "No members found.\n");
    let error = run_team_member_list(&table_cli, &environment, " ", OutputFormat::Json)
        .await
        .expect_err("empty team ID");
    assert_eq!(error.to_string(), "team ID must not be empty");
    server.abort();
}

#[tokio::test]
async fn team_member_add_validates_and_posts_go_compatible_body() {
    let app = Router::new().route(
        "/api/teams/team-1/members",
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
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");

    let defaults = Cli::try_parse_from([
        "patchbay",
        "team",
        "member",
        "add",
        "team-1",
        "--member-id",
        "agent-1",
    ])
    .expect("team member add defaults CLI");
    let Command::Team(TeamArgs {
        command:
            TeamCommand::Member(TeamMemberArgs {
                command:
                    TeamMemberCommand::Add(TeamMemberAddArgs {
                        member_type,
                        role,
                        output,
                        ..
                    }),
            }),
    }) = &defaults.command
    else {
        panic!("expected team member add");
    };
    assert_eq!(member_type, "agent");
    assert_eq!(role, "member");
    assert_eq!(*output, OutputFormat::Json);

    let table_cli = Cli::try_parse_from([
        "patchbay",
        "team",
        "member",
        "add",
        "team-1",
        "--member-id",
        "user-1",
        "--type",
        "member",
        "--role",
        "maintainer",
        "--output",
        "table",
    ])
    .expect("team member add table CLI");
    let table = run_with_input(&table_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("add team member table");
    assert!(table.stdout.is_empty());
    assert_eq!(table.stderr, "Member user-1 added to team.\n");

    let json = run_with_input(&defaults, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("add team member JSON");
    let result: Value = serde_json::from_str(&json.stdout).expect("member JSON");
    assert_eq!(result["member_id"], "agent-1");
    assert!(json.stderr.is_empty());

    let missing = TeamMemberAddArgs {
        team_id: "team-1".into(),
        member_id: None,
        member_type: "agent".into(),
        role: "member".into(),
        output: OutputFormat::Json,
    };
    let error = run_team_member_add(&defaults, &environment, &missing)
        .await
        .expect_err("missing member id");
    assert_eq!(error.to_string(), "--member-id is required");
    let invalid_type = TeamMemberAddArgs {
        member_id: Some("agent-1".into()),
        member_type: "owner".into(),
        ..missing
    };
    let error = run_team_member_add(&defaults, &environment, &invalid_type)
        .await
        .expect_err("invalid member type");
    assert_eq!(error.to_string(), "--type must be 'agent' or 'member'");
    server.abort();
}

#[tokio::test]
async fn team_member_set_role_validates_and_patches_go_compatible_body() {
    let app = Router::new().route(
        "/api/teams/team-1/members/role",
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
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");

    let table_cli = Cli::try_parse_from([
        "patchbay",
        "team",
        "member",
        "set-role",
        "team-1",
        "--member-id",
        "user-1",
        "--member-type",
        "member",
        "--role",
        "lead",
        "--output",
        "table",
    ])
    .expect("team member set-role table CLI");
    let Command::Team(TeamArgs {
        command:
            TeamCommand::Member(TeamMemberArgs {
                command: TeamMemberCommand::SetRole(args),
            }),
    }) = &table_cli.command
    else {
        panic!("expected team member set-role");
    };
    assert_eq!(args.member_type, "member");
    assert_eq!(args.role.as_deref(), Some("lead"));
    assert_eq!(args.output, OutputFormat::Table);
    let table = run_with_input(&table_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("set team member role table");
    assert!(table.stdout.is_empty());
    assert_eq!(table.stderr, "Member user-1 role updated to lead.\n");

    let defaults = Cli::try_parse_from([
        "patchbay",
        "team",
        "member",
        "set-role",
        "team-1",
        "--member-id",
        "agent-1",
        "--role",
        "owner",
    ])
    .expect("team member set-role defaults CLI");
    let Command::Team(TeamArgs {
        command:
            TeamCommand::Member(TeamMemberArgs {
                command: TeamMemberCommand::SetRole(args),
            }),
    }) = &defaults.command
    else {
        panic!("expected team member set-role defaults");
    };
    assert_eq!(args.member_type, "agent");
    assert_eq!(args.output, OutputFormat::Json);
    let json = run_with_input(&defaults, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("set team member role JSON");
    let result: Value = serde_json::from_str(&json.stdout).expect("set role JSON");
    assert_eq!(result["member_id"], "agent-1");
    assert!(json.stderr.is_empty());

    let missing = TeamMemberSetRoleArgs {
        team_id: "team-1".into(),
        member_id: None,
        member_type: "agent".into(),
        role: None,
        output: OutputFormat::Json,
    };
    let error = run_team_member_set_role(&defaults, &environment, &missing)
        .await
        .expect_err("missing member id");
    assert_eq!(error.to_string(), "--member-id is required");
    let missing_role = TeamMemberSetRoleArgs {
        member_id: Some("agent-1".into()),
        ..missing
    };
    let error = run_team_member_set_role(&defaults, &environment, &missing_role)
        .await
        .expect_err("missing role");
    assert_eq!(error.to_string(), "--role is required");
    let invalid_type = TeamMemberSetRoleArgs {
        member_id: Some("agent-1".into()),
        member_type: "owner".into(),
        role: Some("lead".into()),
        ..missing_role
    };
    let error = run_team_member_set_role(&defaults, &environment, &invalid_type)
        .await
        .expect_err("invalid member type");
    assert_eq!(
        error.to_string(),
        "--member-type must be 'agent' or 'member'"
    );
    server.abort();
}

#[tokio::test]
async fn team_member_remove_validates_and_deletes_with_go_compatible_body() {
    let app = Router::new().route(
        "/api/teams/team-1/members",
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
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");

    let table_cli = Cli::try_parse_from([
        "patchbay",
        "team",
        "member",
        "remove",
        "team-1",
        "--member-id",
        "user-1",
        "--type",
        "member",
        "--output",
        "table",
    ])
    .expect("team member remove table CLI");
    let Command::Team(TeamArgs {
        command:
            TeamCommand::Member(TeamMemberArgs {
                command: TeamMemberCommand::Remove(args),
            }),
    }) = &table_cli.command
    else {
        panic!("expected team member remove");
    };
    assert_eq!(args.member_type, "member");
    assert_eq!(args.output, OutputFormat::Table);
    let table = run_with_input(&table_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("remove team member table");
    assert!(table.stdout.is_empty());
    assert_eq!(table.stderr, "Member user-1 removed from team.\n");

    let defaults = Cli::try_parse_from([
        "patchbay",
        "team",
        "member",
        "remove",
        "team-1",
        "--member-id",
        "agent-1",
    ])
    .expect("team member remove defaults CLI");
    let Command::Team(TeamArgs {
        command:
            TeamCommand::Member(TeamMemberArgs {
                command: TeamMemberCommand::Remove(args),
            }),
    }) = &defaults.command
    else {
        panic!("expected team member remove defaults");
    };
    assert_eq!(args.member_type, "agent");
    assert_eq!(args.output, OutputFormat::Json);
    let json = run_with_input(&defaults, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("remove team member JSON");
    let result: Value = serde_json::from_str(&json.stdout).expect("remove JSON");
    assert_eq!(
        result,
        serde_json::json!({
            "team_id": "team-1",
            "member_id": "agent-1",
            "removed": true
        })
    );
    assert!(json.stderr.is_empty());

    let missing = TeamMemberRemoveArgs {
        team_id: "team-1".into(),
        member_id: None,
        member_type: "agent".into(),
        output: OutputFormat::Json,
    };
    let error = run_team_member_remove(&defaults, &environment, &missing)
        .await
        .expect_err("missing member id");
    assert_eq!(error.to_string(), "--member-id is required");
    let invalid_type = TeamMemberRemoveArgs {
        member_id: Some("agent-1".into()),
        member_type: "owner".into(),
        ..missing
    };
    let error = run_team_member_remove(&defaults, &environment, &invalid_type)
        .await
        .expect_err("invalid member type");
    assert_eq!(error.to_string(), "--type must be 'agent' or 'member'");
    let empty_team = TeamMemberRemoveArgs {
        team_id: " ".into(),
        member_id: Some("agent-1".into()),
        ..invalid_type
    };
    let error = run_team_member_remove(&defaults, &environment, &empty_team)
        .await
        .expect_err("empty team id");
    assert_eq!(error.to_string(), "team ID must not be empty");
    server.abort();
}

#[tokio::test]
async fn team_activity_resolves_issue_and_posts_outcome() {
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
            "/api/issues/issue-uuid-18/team-evaluated",
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
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");

    let table_cli = Cli::try_parse_from([
        "patchbay",
        "team",
        "activity",
        "CORD-18",
        "action",
        "--reason",
        "delegated",
    ])
    .expect("team activity table CLI");
    let Command::Team(TeamArgs {
        command: TeamCommand::Activity(args),
    }) = &table_cli.command
    else {
        panic!("expected team activity");
    };
    assert_eq!(args.reason, "delegated");
    assert_eq!(args.output, OutputFormat::Table);
    let table = run_with_input(&table_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("record team activity table");
    assert!(table.stdout.is_empty());
    assert_eq!(
        table.stderr,
        "Team evaluation recorded: action (issue CORD-18)\n"
    );

    let json_cli = Cli::try_parse_from([
        "patchbay",
        "team",
        "activity",
        "CORD-18",
        "no_action",
        "--output",
        "json",
    ])
    .expect("team activity JSON CLI");
    let Command::Team(TeamArgs {
        command: TeamCommand::Activity(args),
    }) = &json_cli.command
    else {
        panic!("expected team activity JSON");
    };
    assert_eq!(args.reason, "");
    assert_eq!(args.output, OutputFormat::Json);
    let json = run_with_input(&json_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("record team activity JSON");
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
        "Team evaluation recorded: no_action (issue CORD-18)\n"
    );

    let invalid = TeamActivityArgs {
        issue_id: "CORD-18".into(),
        outcome: "retry".into(),
        reason: String::new(),
        output: OutputFormat::Table,
    };
    let error = run_team_activity(&table_cli, &environment, &invalid)
        .await
        .expect_err("invalid outcome");
    assert_eq!(
        error.to_string(),
        "invalid outcome \"retry\"; valid values: action, no_action, failed"
    );
    server.abort();
}
