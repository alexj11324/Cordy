use super::*;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete as delete_route, get, post, put};
use axum::{Json, Router};
use clap::Parser;
use std::fs;
use std::io::Cursor;
use tokio::net::TcpListener;

#[tokio::test]
async fn skill_list_matches_go_requests_and_table_json_outputs() {
    let app = Router::new().route(
        "/api/skills",
        get(|headers: HeaderMap| async move {
            assert_eq!(headers["authorization"], "Bearer token-1");
            assert_eq!(headers["x-workspace-id"], "workspace-1");
            Json(vec![serde_json::json!({
                "id": "skill-1",
                "name": "Reviewer",
                "description": "Reviews changes",
                "created_at": "2026-08-24T00:00:00Z",
                "server_only": "preserved"
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
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");

    let table_cli =
        Cli::try_parse_from(["patchbay", "skill", "list"]).expect("skill list table CLI");
    let Command::Skill(SkillArgs {
        command: SkillCommand::List { output },
    }) = &table_cli.command
    else {
        panic!("expected skill list");
    };
    assert_eq!(*output, OutputFormat::Table);
    let table = run_with_input(&table_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list skills table");
    assert!(table.stdout.starts_with("ID"));
    assert!(table.stdout.contains("NAME"));
    assert!(table.stdout.contains("DESCRIPTION"));
    assert!(table.stdout.contains("CREATED_AT"));
    assert!(table.stdout.contains("skill-1"));
    assert!(table.stdout.contains("Reviewer"));
    assert!(table.stderr.is_empty());

    let json_cli = Cli::try_parse_from(["patchbay", "skill", "list", "--output", "json"])
        .expect("skill list JSON CLI");
    let json = run_with_input(&json_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list skills JSON");
    let skills: Value = serde_json::from_str(&json.stdout).expect("skills JSON");
    assert_eq!(skills[0]["server_only"], "preserved");
    assert!(json.stderr.is_empty());

    let empty = format_skill_list_table(&[]);
    assert!(empty.starts_with("ID"));
    assert!(empty.contains("CREATED_AT"));
    server.abort();
}

#[tokio::test]
async fn skill_get_matches_go_path_headers_and_output_contracts() {
    let app = Router::new().route(
        "/api/skills/skill-1",
        get(|headers: HeaderMap| async move {
            assert_eq!(headers["authorization"], "Bearer token-1");
            assert_eq!(headers["x-workspace-id"], "workspace-1");
            Json(serde_json::json!({
                "id": "skill-1",
                "name": "Reviewer",
                "description": "Reviews changes",
                "created_at": "2026-08-24T00:00:00Z",
                "server_only": "preserved"
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

    let defaults = Cli::try_parse_from(["patchbay", "skill", "get", "skill-1"])
        .expect("skill get default CLI");
    let Command::Skill(SkillArgs {
        command: SkillCommand::Get(args),
    }) = &defaults.command
    else {
        panic!("expected skill get");
    };
    assert_eq!(args.output, OutputFormat::Json);
    let json = run_with_input(&defaults, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("get skill JSON");
    let skill: Value = serde_json::from_str(&json.stdout).expect("skill JSON");
    assert_eq!(skill["server_only"], "preserved");
    assert!(json.stderr.is_empty());

    let table_cli =
        Cli::try_parse_from(["patchbay", "skill", "get", "skill-1", "--output", "table"])
            .expect("skill get table CLI");
    let table = run_with_input(&table_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("get skill table");
    assert!(table.stdout.starts_with("ID"));
    assert!(table.stdout.contains("NAME"));
    assert!(table.stdout.contains("DESCRIPTION"));
    assert!(table.stdout.contains("CREATED_AT"));
    assert!(table.stdout.contains("Reviews changes"));
    assert!(table.stderr.is_empty());

    let empty = SkillGetArgs {
        skill_id: " ".into(),
        output: OutputFormat::Json,
    };
    let error = run_skill_get(&defaults, &environment, &empty)
        .await
        .expect_err("empty skill ID");
    assert_eq!(error.to_string(), "skill ID must not be empty");
    server.abort();
}

#[tokio::test]
async fn skill_create_matches_go_content_config_and_output_contracts() {
    let app = Router::new().route(
        "/api/skills",
        post(|headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["authorization"], "Bearer token-1");
            assert_eq!(headers["x-workspace-id"], "workspace-1");
            match body["name"].as_str() {
                Some("Reviewer") => {
                    assert_eq!(body["description"], "Reviews changes");
                    assert_eq!(body["content"], "line1\nline2\n");
                    assert_eq!(body["config"], serde_json::json!({"level":"strict"}));
                }
                Some("Inline") => {
                    assert_eq!(body["content"], "inline");
                    assert!(body.get("description").is_none());
                    assert!(body.get("config").is_none());
                }
                other => panic!("unexpected skill name: {other:?}"),
            }
            Json(serde_json::json!({
                "id": "skill-1",
                "name": body["name"].clone()
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

    let table_cli = Cli::try_parse_from([
        "patchbay",
        "skill",
        "create",
        "--name",
        "Reviewer",
        "--description",
        "Reviews changes",
        "--content-stdin",
        "--config",
        r#"{"level":"strict"}"#,
        "--output",
        "table",
    ])
    .expect("skill create table CLI");
    let Command::Skill(SkillArgs {
        command: SkillCommand::Create(args),
    }) = &table_cli.command
    else {
        panic!("expected skill create");
    };
    assert_eq!(args.output, OutputFormat::Table);
    let table = run_with_input(
        &table_cli,
        &environment,
        &mut Cursor::new(b"line1\nline2\n".to_vec()),
    )
    .await
    .expect("create skill table");
    assert_eq!(table.stdout, "Skill created: Reviewer (skill-1)\n");
    assert!(table.stderr.is_empty());

    let json_cli = Cli::try_parse_from([
        "patchbay",
        "skill",
        "create",
        "--name",
        "Inline",
        "--content",
        "inline",
    ])
    .expect("skill create JSON CLI");
    let Command::Skill(SkillArgs {
        command: SkillCommand::Create(args),
    }) = &json_cli.command
    else {
        panic!("expected skill create JSON");
    };
    assert_eq!(args.output, OutputFormat::Json);
    let json = run_with_input(&json_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("create skill JSON");
    let created: Value = serde_json::from_str(&json.stdout).expect("created skill JSON");
    assert_eq!(created["name"], "Inline");
    assert_eq!(created["id"], "skill-1");
    assert!(json.stderr.is_empty());

    let missing_name = SkillCreateArgs {
        name: None,
        description: String::new(),
        content: None,
        content_stdin: false,
        content_file: None,
        config: None,
        output: OutputFormat::Json,
    };
    let error = run_skill_create(
        &json_cli,
        &environment,
        &missing_name,
        &mut Cursor::new(Vec::<u8>::new()),
    )
    .await
    .expect_err("missing skill name");
    assert_eq!(error.to_string(), "--name is required");

    let conflict = SkillCreateArgs {
        name: Some("Conflict".into()),
        content: Some("inline".into()),
        content_stdin: true,
        ..missing_name
    };
    let error = resolve_skill_content(&conflict, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .expect_err("conflicting content sources");
    assert_eq!(
        error.to_string(),
        "--content, --content-stdin, and --content-file are mutually exclusive"
    );

    let empty_stdin = SkillCreateArgs {
        name: Some("Empty".into()),
        description: String::new(),
        content: None,
        content_stdin: true,
        content_file: None,
        config: None,
        output: OutputFormat::Json,
    };
    assert_eq!(
        resolve_skill_content(
            &empty_stdin,
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect("empty stdin is accepted"),
        Some(String::new())
    );

    let invalid_config = SkillCreateArgs {
        name: Some("Invalid".into()),
        description: String::new(),
        content: None,
        content_stdin: false,
        content_file: None,
        config: Some("{".into()),
        output: OutputFormat::Json,
    };
    let error = run_skill_create(
        &json_cli,
        &environment,
        &invalid_config,
        &mut Cursor::new(Vec::<u8>::new()),
    )
    .await
    .expect_err("invalid config");
    assert!(error
        .to_string()
        .starts_with("--config must be valid JSON:"));
    server.abort();
}

#[tokio::test]
async fn skill_update_matches_go_put_body_and_output_contracts() {
    let app = Router::new().route(
        "/api/skills/skill-1",
        put(|headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["authorization"], "Bearer token-1");
            assert_eq!(headers["x-workspace-id"], "workspace-1");
            match body["name"].as_str() {
                Some("Reviewer") => {
                    assert_eq!(body["description"], "");
                    assert_eq!(body["content"], "");
                    assert_eq!(body["config"], serde_json::json!({"strict":true}));
                }
                Some("Inline") => {
                    assert_eq!(body["content"], "inline");
                    assert!(body.get("description").is_none());
                    assert!(body.get("config").is_none());
                }
                other => panic!("unexpected skill name: {other:?}"),
            }
            Json(serde_json::json!({
                "id": "skill-1",
                "name": body["name"].clone()
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

    let table_cli = Cli::try_parse_from([
        "patchbay",
        "skill",
        "update",
        "skill-1",
        "--name",
        "Reviewer",
        "--description",
        "",
        "--content-stdin",
        "--config",
        r#"{"strict":true}"#,
        "--output",
        "table",
    ])
    .expect("skill update table CLI");
    let Command::Skill(SkillArgs {
        command: SkillCommand::Update(args),
    }) = &table_cli.command
    else {
        panic!("expected skill update");
    };
    assert_eq!(args.description.as_deref(), Some(""));
    assert_eq!(args.output, OutputFormat::Table);
    let table = run_with_input(&table_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("update skill table");
    assert_eq!(table.stdout, "Skill updated: Reviewer (skill-1)\n");
    assert!(table.stderr.is_empty());

    let json_cli = Cli::try_parse_from([
        "patchbay",
        "skill",
        "update",
        "skill-1",
        "--name",
        "Inline",
        "--content",
        "inline",
    ])
    .expect("skill update JSON CLI");
    let Command::Skill(SkillArgs {
        command: SkillCommand::Update(args),
    }) = &json_cli.command
    else {
        panic!("expected skill update JSON");
    };
    assert_eq!(args.output, OutputFormat::Json);
    let json = run_with_input(&json_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("update skill JSON");
    let updated: Value = serde_json::from_str(&json.stdout).expect("updated skill JSON");
    assert_eq!(updated["name"], "Inline");
    assert_eq!(updated["id"], "skill-1");
    assert!(json.stderr.is_empty());

    let no_fields = SkillUpdateArgs {
        skill_id: "skill-1".into(),
        name: None,
        description: None,
        content: None,
        content_stdin: false,
        content_file: None,
        config: None,
        output: OutputFormat::Json,
    };
    let error = run_skill_update(
        &json_cli,
        &environment,
        &no_fields,
        &mut Cursor::new(Vec::<u8>::new()),
    )
    .await
    .expect_err("no fields to update");
    assert_eq!(
        error.to_string(),
        "no fields to update; use --name, --description, --content, or --config"
    );

    let conflict = SkillUpdateArgs {
        skill_id: "skill-1".into(),
        content: Some("inline".into()),
        content_stdin: true,
        ..no_fields
    };
    let error = resolve_skill_content_sources(
        conflict.content.as_deref(),
        conflict.content_stdin,
        conflict.content_file.as_deref(),
        &environment,
        &mut Cursor::new(Vec::<u8>::new()),
    )
    .expect_err("conflicting update content sources");
    assert_eq!(
        error.to_string(),
        "--content, --content-stdin, and --content-file are mutually exclusive"
    );
    server.abort();
}

#[tokio::test]
async fn skill_delete_matches_go_confirmation_and_request_contracts() {
    let app = Router::new().route(
        "/api/skills/skill-1",
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

    let confirmed = Cli::try_parse_from(["patchbay", "skill", "delete", "skill-1"])
        .expect("skill delete confirmation CLI");
    let Command::Skill(SkillArgs {
        command: SkillCommand::Delete(args),
    }) = &confirmed.command
    else {
        panic!("expected skill delete");
    };
    assert!(!args.yes);
    let output = run_with_input(
        &confirmed,
        &environment,
        &mut Cursor::new(b"YeS\n".to_vec()),
    )
    .await
    .expect("confirmed skill delete");
    assert_eq!(
        output.stdout,
        "Are you sure you want to delete skill skill-1? This cannot be undone. [y/N] Skill deleted: skill-1\n"
    );
    assert!(output.stderr.is_empty());

    let yes_cli = Cli::try_parse_from(["patchbay", "skill", "delete", "skill-1", "--yes"])
        .expect("skill delete --yes CLI");
    let Command::Skill(SkillArgs {
        command: SkillCommand::Delete(args),
    }) = &yes_cli.command
    else {
        panic!("expected skill delete --yes");
    };
    assert!(args.yes);
    let output = run_with_input(&yes_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("skill delete --yes");
    assert_eq!(output.stdout, "Skill deleted: skill-1\n");
    assert!(output.stderr.is_empty());

    let declined = run_with_input(&confirmed, &environment, &mut Cursor::new(b"n\n".to_vec()))
        .await
        .expect("declined skill delete");
    assert_eq!(
        declined.stdout,
        "Are you sure you want to delete skill skill-1? This cannot be undone. [y/N] Aborted.\n"
    );
    assert!(declined.stderr.is_empty());

    let eof = run_with_input(&confirmed, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("EOF skill delete");
    assert_eq!(
        eof.stdout,
        "Are you sure you want to delete skill skill-1? This cannot be undone. [y/N] Aborted.\n"
    );
    assert!(eof.stderr.is_empty());

    let empty = SkillDeleteArgs {
        skill_id: " ".into(),
        yes: true,
    };
    let error = run_skill_delete(
        &yes_cli,
        &environment,
        &empty,
        &mut Cursor::new(Vec::<u8>::new()),
    )
    .await
    .expect_err("empty skill id");
    assert_eq!(error.to_string(), "skill ID must not be empty");
    server.abort();
}

#[tokio::test]
async fn skill_import_matches_go_url_and_multipart_contracts() {
    let app = Router::new().route(
        "/api/skills/import",
        post(|request: Request| async move {
            assert_eq!(request.headers()["authorization"], "Bearer token-1");
            assert_eq!(request.headers()["x-workspace-id"], "workspace-1");
            let content_type = request
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            let body = axum::body::to_bytes(request.into_body(), 2 << 20)
                .await
                .expect("import request body");
            let body_text = String::from_utf8_lossy(&body);
            if content_type == "application/json" {
                let body: Value = serde_json::from_slice(&body).expect("URL import JSON");
                assert_eq!(body["url"], "https://skills.sh/acme/repo/reviewer");
                assert_eq!(body["on_conflict"], "rename");
            } else {
                assert!(content_type.starts_with("multipart/form-data; boundary="));
                assert!(body_text.contains("name=\"file\"; filename=\"review.skill\""));
                assert!(body_text.contains("name=\"on_conflict\""));
                assert!(body_text.contains("\r\n\r\nskip\r\n"));
                assert!(body_text.contains("archive bytes"));
            }
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "status": "created",
                    "skill": {"id": "skill-1", "name": "Reviewer"}
                })),
            )
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    fs::write(cwd.path().join("review.skill"), b"archive bytes").expect("skill archive");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");

    let url_cli = Cli::try_parse_from([
        "patchbay",
        "skill",
        "import",
        "--url",
        "https://skills.sh/acme/repo/reviewer",
        "--on-conflict",
        "rename",
        "--output",
        "table",
    ])
    .expect("URL import CLI");
    let url_output = run_with_input(&url_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("URL import");
    assert_eq!(url_output.stdout, "Skill imported: Reviewer (skill-1)\n");
    assert!(url_output.stderr.is_empty());

    let file_cli = Cli::try_parse_from([
        "patchbay",
        "skill",
        "import",
        "--file",
        "review.skill",
        "--on-conflict",
        "skip",
    ])
    .expect("archive import CLI");
    let file_output = run_with_input(&file_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("archive import");
    let file_json: Value = serde_json::from_str(&file_output.stdout).expect("archive JSON");
    assert_eq!(file_json["status"], "created");
    assert!(file_output.stderr.is_empty());
    server.abort();
}

#[tokio::test]
async fn skill_import_rejects_ambiguous_input_and_preserves_legacy_conflict_output() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let cli = Cli::try_parse_from([
        "patchbay",
        "skill",
        "import",
        "--url",
        "https://skills.sh/acme/repo/reviewer",
        "--file",
        "review.skill",
    ])
    .expect("ambiguous import CLI");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("ambiguous import");
    assert_eq!(error.to_string(), "--url and --file are mutually exclusive");

    let invalid = Cli::try_parse_from([
        "patchbay",
        "skill",
        "import",
        "--url",
        "https://skills.sh/acme/repo/reviewer",
        "--on-conflict",
        "merge",
    ])
    .expect("invalid conflict strategy reaches runtime");
    let error = run_with_input(&invalid, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("invalid conflict strategy");
    assert_eq!(
        error.to_string(),
        "--on-conflict must be one of: fail, overwrite, rename, skip"
    );

    let archive = cwd.path().join("review.skill");
    fs::write(&archive, b"archive").expect("archive");
    let outside = tempfile::tempdir().expect("outside");
    let outside_file = outside.path().join("review.skill");
    fs::write(&outside_file, b"outside").expect("outside archive");
    let error = read_skill_archive(&outside_file, &environment, false)
        .expect_err("external archive must be refused");
    assert!(error.to_string().contains("--file path"));
    let (bytes, filename) =
        read_skill_archive(&archive, &environment, false).expect("workdir archive");
    assert_eq!(bytes, b"archive");
    assert_eq!(filename, "review.skill");
}

#[tokio::test]
async fn skill_import_legacy_conflict_is_structured_and_nonzero() {
    let app = Router::new().route(
        "/api/skills/import",
        post(|| async {
            (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "a skill with this name already exists",
                    "existing_skill": {"id": "skill-1", "name": "Reviewer"}
                })),
            )
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
        "skill",
        "import",
        "--url",
        "https://skills.sh/acme/repo/reviewer",
    ])
    .expect("legacy conflict CLI");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("legacy conflict");
    let output = command_error_output(&error).expect("structured conflict output");
    let result: Value = serde_json::from_str(&output.stdout).expect("conflict JSON");
    assert_eq!(result["status"], "conflict");
    assert_eq!(result["existing_skill"]["id"], "skill-1");
    assert!(result["reason"]
        .as_str()
        .expect("reason")
        .contains("--on-conflict overwrite"));
    assert_eq!(crate::error::exit_code(&error), 1);
    server.abort();
}

#[test]
fn skill_import_table_covers_structured_statuses() {
    assert_eq!(
        format_skill_import_table(&serde_json::json!({
            "status": "updated",
            "reason": "refreshed",
            "skill": {"name": "Reviewer", "id": "skill-1"}
        })),
        "Skill updated: Reviewer (skill-1)\nReason: refreshed\n"
    );
    assert_eq!(
        format_skill_import_table(&serde_json::json!({
            "status": "skipped",
            "existing_skill": {"name": "Reviewer", "id": "skill-1"}
        })),
        "Skill skipped: Reviewer (skill-1)\n"
    );
    assert_eq!(
        format_skill_import_table(&serde_json::json!({
            "status": "failed",
            "reason": "source unavailable"
        })),
        "Skill import failed: source unavailable\n"
    );
}

#[tokio::test]
async fn skill_refresh_matches_go_body_headers_and_output_contracts() {
    let app = Router::new().route(
        "/api/skills/skill-1/refresh",
        post(|headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["authorization"], "Bearer token-1");
            assert_eq!(headers["x-workspace-id"], "workspace-1");
            assert_eq!(body, serde_json::json!({}));
            Json(serde_json::json!({
                "id": "skill-1",
                "name": "Reviewer",
                "description": "Refreshed"
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

    let defaults = Cli::try_parse_from(["patchbay", "skill", "refresh", "skill-1"])
        .expect("skill refresh default CLI");
    let Command::Skill(SkillArgs {
        command: SkillCommand::Refresh(args),
    }) = &defaults.command
    else {
        panic!("expected skill refresh");
    };
    assert_eq!(args.output, OutputFormat::Json);
    let json = run_with_input(&defaults, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("refresh skill JSON");
    let refreshed: Value = serde_json::from_str(&json.stdout).expect("refreshed skill JSON");
    assert_eq!(refreshed["name"], "Reviewer");
    assert_eq!(refreshed["id"], "skill-1");
    assert!(json.stderr.is_empty());

    let table_cli = Cli::try_parse_from([
        "patchbay", "skill", "refresh", "skill-1", "--output", "table",
    ])
    .expect("skill refresh table CLI");
    let table = run_with_input(&table_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("refresh skill table");
    assert_eq!(
        table.stdout,
        "Skill updated from source: Reviewer (skill-1)\n"
    );
    assert!(table.stderr.is_empty());

    let empty = SkillRefreshArgs {
        skill_id: " ".into(),
        output: OutputFormat::Json,
    };
    let error = run_skill_refresh(&defaults, &environment, &empty)
        .await
        .expect_err("empty skill ID");
    assert_eq!(error.to_string(), "skill ID must not be empty");
    server.abort();
}

#[tokio::test]
async fn skill_search_matches_go_query_headers_and_output_contracts() {
    let app = Router::new().route(
        "/api/skills/search",
        get(|request: Request| async move {
            assert_eq!(request.headers()["authorization"], "Bearer token-1");
            assert_eq!(request.headers()["x-workspace-id"], "workspace-1");
            assert_eq!(request.uri().query(), Some("q=Rust+Review"));
            Json(vec![serde_json::json!({
                "name": "Reviewer",
                "url": "https://skills.example/reviewer",
                "source": "skills.sh",
                "install_count": 12,
                "description": "Reviews changes"
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
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");

    let table_cli = Cli::try_parse_from([
        "patchbay",
        "skill",
        "search",
        "Rust Review",
        "--output",
        "table",
    ])
    .expect("skill search table CLI");
    let Command::Skill(SkillArgs {
        command: SkillCommand::Search(args),
    }) = &table_cli.command
    else {
        panic!("expected skill search");
    };
    assert_eq!(args.query, "Rust Review");
    assert_eq!(args.output, OutputFormat::Table);
    let table = run_with_input(&table_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("search skills table");
    assert!(table.stdout.starts_with("NAME"));
    assert!(table.stdout.contains("URL"));
    assert!(table.stdout.contains("SOURCE"));
    assert!(table.stdout.contains("INSTALLS"));
    assert!(table.stdout.contains("DESCRIPTION"));
    assert!(table.stdout.contains("Reviewer"));
    assert!(table.stdout.contains("12"));
    assert!(table.stderr.is_empty());

    let json_cli = Cli::try_parse_from(["patchbay", "skill", "search", "Rust Review"])
        .expect("skill search JSON CLI");
    let json = run_with_input(&json_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("search skills JSON");
    let results: Value = serde_json::from_str(&json.stdout).expect("search JSON");
    assert_eq!(results[0]["install_count"], 12);
    assert!(json.stderr.is_empty());

    let empty = SkillSearchArgs {
        query: "  ".into(),
        output: OutputFormat::Json,
    };
    let error = run_skill_search(&json_cli, &environment, &empty)
        .await
        .expect_err("empty search query");
    assert_eq!(error.to_string(), "query is required");
    let empty_table = format_skill_search_table(&[]);
    assert!(empty_table.starts_with("NAME"));
    assert!(empty_table.contains("DESCRIPTION"));
    server.abort();
}

#[tokio::test]
async fn skill_files_list_matches_go_path_headers_and_output_contracts() {
    let app = Router::new().route(
        "/api/skills/skill-1/files",
        get(|headers: HeaderMap| async move {
            assert_eq!(headers["authorization"], "Bearer token-1");
            assert_eq!(headers["x-workspace-id"], "workspace-1");
            Json(vec![serde_json::json!({
                "id": "file-1",
                "path": "SKILL.md",
                "created_at": "2026-08-24T00:00:00Z",
                "updated_at": "2026-08-24T01:00:00Z",
                "content": "not printed"
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
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");

    let table_cli = Cli::try_parse_from(["patchbay", "skill", "files", "list", "skill-1"])
        .expect("skill files list table CLI");
    let Command::Skill(SkillArgs {
        command:
            SkillCommand::Files(SkillFilesArgs {
                command: SkillFilesCommand::List(args),
            }),
    }) = &table_cli.command
    else {
        panic!("expected skill files list");
    };
    assert_eq!(args.output, OutputFormat::Table);
    let table = run_with_input(&table_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list skill files table");
    assert!(table.stdout.starts_with("ID"));
    assert!(table.stdout.contains("PATH"));
    assert!(table.stdout.contains("CREATED_AT"));
    assert!(table.stdout.contains("UPDATED_AT"));
    assert!(table.stdout.contains("SKILL.md"));
    assert!(!table.stdout.contains("not printed"));
    assert!(table.stderr.is_empty());

    let json_cli = Cli::try_parse_from([
        "patchbay", "skill", "files", "list", "skill-1", "--output", "json",
    ])
    .expect("skill files list JSON CLI");
    let json = run_with_input(&json_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list skill files JSON");
    let files: Value = serde_json::from_str(&json.stdout).expect("skill files JSON");
    assert_eq!(files[0]["content"], "not printed");
    assert!(json.stderr.is_empty());

    let empty = SkillFilesListArgs {
        skill_id: " ".into(),
        output: OutputFormat::Json,
    };
    let error = run_skill_files_list(&json_cli, &environment, &empty)
        .await
        .expect_err("empty skill ID");
    assert_eq!(error.to_string(), "skill ID must not be empty");
    let empty_table = format_skill_files_table(&[]);
    assert!(empty_table.starts_with("ID"));
    assert!(empty_table.contains("UPDATED_AT"));
    server.abort();
}

#[tokio::test]
async fn skill_files_upsert_matches_go_body_headers_and_output_contracts() {
    let app = Router::new().route(
        "/api/skills/skill-1/files",
        put(|headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["authorization"], "Bearer token-1");
            assert_eq!(headers["x-workspace-id"], "workspace-1");
            match body["path"].as_str() {
                Some("SKILL.md") => assert_eq!(body["content"], "body\n"),
                Some("notes.md") => assert_eq!(body["content"], "inline"),
                other => panic!("unexpected skill file path: {other:?}"),
            }
            Json(serde_json::json!({
                "id": "file-1",
                "path": body["path"].clone(),
                "content": body["content"].clone()
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

    let table_cli = Cli::try_parse_from([
        "patchbay",
        "skill",
        "files",
        "upsert",
        "skill-1",
        "--path",
        "SKILL.md",
        "--content-stdin",
        "--output",
        "table",
    ])
    .expect("skill file upsert table CLI");
    let Command::Skill(SkillArgs {
        command:
            SkillCommand::Files(SkillFilesArgs {
                command: SkillFilesCommand::Upsert(args),
            }),
    }) = &table_cli.command
    else {
        panic!("expected skill file upsert");
    };
    assert_eq!(args.path.as_deref(), Some("SKILL.md"));
    assert_eq!(args.output, OutputFormat::Table);
    let table = run_with_input(
        &table_cli,
        &environment,
        &mut Cursor::new(b"body\n".to_vec()),
    )
    .await
    .expect("upsert skill file table");
    assert_eq!(table.stdout, "Skill file upserted: SKILL.md (file-1)\n");
    assert!(table.stderr.is_empty());

    let json_cli = Cli::try_parse_from([
        "patchbay",
        "skill",
        "files",
        "upsert",
        "skill-1",
        "--path",
        "notes.md",
        "--content",
        "inline",
    ])
    .expect("skill file upsert JSON CLI");
    let json = run_with_input(&json_cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("upsert skill file JSON");
    let result: Value = serde_json::from_str(&json.stdout).expect("upsert JSON");
    assert_eq!(result["id"], "file-1");
    assert_eq!(result["path"], "notes.md");
    assert!(json.stderr.is_empty());

    let missing_path = SkillFilesUpsertArgs {
        skill_id: "skill-1".into(),
        path: None,
        content: Some("inline".into()),
        content_stdin: false,
        content_file: None,
        output: OutputFormat::Json,
    };
    let error = run_skill_files_upsert(
        &json_cli,
        &environment,
        &missing_path,
        &mut Cursor::new(Vec::<u8>::new()),
    )
    .await
    .expect_err("missing path");
    assert_eq!(error.to_string(), "--path is required");
    let missing_content = SkillFilesUpsertArgs {
        path: Some("notes.md".into()),
        content: None,
        ..missing_path
    };
    let error = run_skill_files_upsert(
        &json_cli,
        &environment,
        &missing_content,
        &mut Cursor::new(Vec::<u8>::new()),
    )
    .await
    .expect_err("missing content");
    assert_eq!(error.to_string(), "--content is required");
    let conflict = SkillFilesUpsertArgs {
        content: Some("inline".into()),
        content_stdin: true,
        ..missing_content
    };
    let error = run_skill_files_upsert(
        &json_cli,
        &environment,
        &conflict,
        &mut Cursor::new(Vec::<u8>::new()),
    )
    .await
    .expect_err("conflicting content");
    assert_eq!(
        error.to_string(),
        "--content, --content-stdin, and --content-file are mutually exclusive"
    );
    server.abort();
}

#[tokio::test]
async fn skill_files_delete_matches_go_path_headers_and_output_contracts() {
    let app = Router::new().route(
        "/api/skills/skill-1/files/file-1",
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

    let cli = Cli::try_parse_from(["patchbay", "skill", "files", "delete", "skill-1", "file-1"])
        .expect("skill file delete CLI");
    let Command::Skill(SkillArgs {
        command:
            SkillCommand::Files(SkillFilesArgs {
                command: SkillFilesCommand::Delete(args),
            }),
    }) = &cli.command
    else {
        panic!("expected skill file delete");
    };
    assert_eq!(args.skill_id, "skill-1");
    assert_eq!(args.file_id, "file-1");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("delete skill file");
    assert_eq!(output.stdout, "Skill file deleted: file-1\n");
    assert!(output.stderr.is_empty());

    let empty_skill = SkillFilesDeleteArgs {
        skill_id: " ".into(),
        file_id: "file-1".into(),
    };
    let error = run_skill_files_delete(&cli, &environment, &empty_skill)
        .await
        .expect_err("empty skill id");
    assert_eq!(error.to_string(), "skill ID must not be empty");
    let empty_file = SkillFilesDeleteArgs {
        skill_id: "skill-1".into(),
        file_id: " ".into(),
    };
    let error = run_skill_files_delete(&cli, &environment, &empty_file)
        .await
        .expect_err("empty file id");
    assert_eq!(error.to_string(), "file ID must not be empty");
    server.abort();
}
