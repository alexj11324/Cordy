use super::*;
use axum::extract::Request;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use clap::Parser;
use std::collections::HashMap;
use std::io::Cursor;
use tokio::net::TcpListener;

#[test]
fn project_read_parser_and_tables_match_go_registry_contract() {
    let cli = Cli::try_parse_from([
        "patchbay",
        "project",
        "list",
        "--status",
        "in_progress",
        "--full-id",
        "--output",
        "json",
    ])
    .expect("project list CLI");
    let Command::Project(ProjectArgs {
        command:
            ProjectCommand::List {
                output,
                full_id,
                status,
            },
    }) = &cli.command
    else {
        panic!("expected project list");
    };
    assert_eq!(*output, OutputFormat::Json);
    assert!(*full_id);
    assert_eq!(status.as_deref(), Some("in_progress"));

    let project = serde_json::json!({
        "id":"11111111-1111-1111-1111-111111111111","title":"Migration",
        "status":"in_progress","lead_type":"member","lead_id":"member-1",
        "created_at":"2026-08-24T12:34:56Z","description":"Rust port"
    });
    let actors = IssueActorNames(HashMap::from([("member:member-1".into(), "Ada".into())]));
    let list = format_project_list_table(std::slice::from_ref(&project), &actors, false);
    assert!(list.starts_with("ID"));
    assert!(list.contains("11111111"));
    assert!(list.contains("Migration"));
    assert!(list.contains("member:Ada"));
    assert!(list.contains("2026-08-24"));
    let details = format_project_details_table(&project, &actors);
    assert!(details.contains("11111111-1111-1111-1111-111111111111"));
    assert!(details.contains("Rust port"));
}

#[tokio::test]
async fn project_list_sends_workspace_status_and_preserves_json_array() {
    let app = Router::new().route(
        "/api/projects",
        get(|request: Request| async move {
            let query = request.uri().query().unwrap_or_default();
            assert!(query.contains("workspace_id=workspace-1"));
            assert!(query.contains("status=in_progress"));
            Json(serde_json::json!({
                "projects":[{"id":"project-1","title":"Migration","status":"in_progress"}]
            }))
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");
    let cli = Cli::try_parse_from([
        "patchbay",
        "project",
        "list",
        "--status",
        "in_progress",
        "--output",
        "json",
    ])
    .expect("project list CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list projects");
    let projects: Value = serde_json::from_str(&output.stdout).expect("projects JSON");
    assert_eq!(projects[0]["title"], "Migration");
    task.abort();
}

#[tokio::test]
async fn project_get_resolves_prefix_and_reports_attached_resources() {
    let project_id = "abcd1234-0000-0000-0000-000000000000";
    let app = Router::new()
        .route(
            "/api/projects",
            get(move || async move {
                Json(serde_json::json!({
                    "projects":[{"id":project_id,"title":"Migration","status":"planned"}]
                }))
            }),
        )
        .route(
            "/api/projects/abcd1234-0000-0000-0000-000000000000",
            get(move || async move {
                Json(serde_json::json!({
                    "id":project_id,"title":"Migration","status":"planned",
                    "description":"Rust port","resource_count":2
                }))
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");
    let cli = Cli::try_parse_from(["patchbay", "project", "get", "abcd", "--output", "table"])
        .expect("project get CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("get project");
    assert!(output.stdout.contains("Migration"));
    assert!(output.stderr.contains("2 resource(s) attached"));
    assert!(output.stderr.contains(project_id));
    task.abort();
}

#[test]
fn project_mutation_parser_and_status_validation_match_go_contract() {
    let create = Cli::try_parse_from([
        "patchbay",
        "project",
        "create",
        "--title",
        "Migration",
        "--status",
        "planned",
        "--repo",
        "https://github.com/acme/one",
        "--repo",
        "https://github.com/acme/two",
    ])
    .expect("project create CLI");
    let Command::Project(ProjectArgs {
        command: ProjectCommand::Create(args),
    }) = &create.command
    else {
        panic!("expected project create");
    };
    assert_eq!(args.repo.len(), 2);
    for status in PROJECT_STATUSES {
        validate_project_status(status).expect("valid project status");
    }
    assert!(validate_project_status("active")
        .expect_err("invalid status")
        .to_string()
        .contains("planned"));

    let update = Cli::try_parse_from([
        "patchbay",
        "project",
        "update",
        "11111111-1111-1111-1111-111111111111",
        "--start-date=",
        "--due-date=",
    ])
    .expect("project update clears");
    let Command::Project(ProjectArgs {
        command: ProjectCommand::Update(args),
    }) = &update.command
    else {
        panic!("expected project update");
    };
    assert_eq!(args.start_date.as_deref(), Some(""));
    assert_eq!(args.due_date.as_deref(), Some(""));
}

#[tokio::test]
async fn project_create_bundles_repos_and_status_updates_return_go_outputs() {
    let project_id = "11111111-1111-1111-1111-111111111111";
    let app = Router::new()
        .route(
            "/api/projects",
            post(|Json(body): Json<Value>| async move {
                assert_eq!(body["title"], "Migration");
                assert_eq!(body["status"], "planned");
                assert_eq!(body["resources"].as_array().expect("resources").len(), 2);
                assert_eq!(
                    body["resources"][0]["resource_ref"]["url"],
                    "https://github.com/acme/one"
                );
                Json(serde_json::json!({
                    "id":"11111111-1111-1111-1111-111111111111",
                    "title":"Migration","status":"planned"
                }))
            }),
        )
        .route(
            "/api/projects/11111111-1111-1111-1111-111111111111",
            put(|Json(body): Json<Value>| async move {
                assert_eq!(body, serde_json::json!({"status":"completed"}));
                Json(serde_json::json!({
                    "id":"11111111-1111-1111-1111-111111111111",
                    "title":"Migration","status":"completed"
                }))
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TOKEN", "token-1");
    let create = Cli::try_parse_from([
        "patchbay",
        "project",
        "create",
        "--title",
        "Migration",
        "--status",
        "planned",
        "--repo",
        "https://github.com/acme/one",
        "--repo",
        "https://github.com/acme/two",
    ])
    .expect("project create CLI");
    let created = run_with_input(&create, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("create project");
    assert_eq!(
        serde_json::from_str::<Value>(&created.stdout).expect("project JSON")["id"],
        project_id
    );

    let status = Cli::try_parse_from([
        "patchbay",
        "project",
        "status",
        project_id,
        "completed",
        "--output",
        "table",
    ])
    .expect("project status CLI");
    let updated = run_with_input(&status, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("update project status");
    assert!(updated.stdout.is_empty());
    assert_eq!(
        updated.stderr,
        "Project Migration status changed to completed.\n"
    );
    task.abort();
}
