use super::*;
use axum::routing::{get, put};
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use tokio::net::TcpListener;

#[test]
fn project_resource_add_parser_and_ref_shortcuts_match_go_contract() {
    let cli = Cli::try_parse_from([
        "cordy",
        "project",
        "resource",
        "add",
        "11111111-1111-1111-1111-111111111111",
        "--url",
        "https://github.com/acme/cordy",
        "--ref",
        "2024",
        "--default-branch-hint",
        "main",
        "--label",
        "Cordy",
    ])
    .expect("project resource add CLI");
    let Command::Project(ProjectArgs {
        command:
            ProjectCommand::Resource(ProjectResourceArgs {
                command: ProjectResourceCommand::Add(args),
            }),
    }) = &cli.command
    else {
        panic!("expected project resource add");
    };
    assert_eq!(args.resource_type, "github_repo");
    assert_eq!(
        build_project_resource_add_ref(args).expect("github ref"),
        serde_json::json!({
            "url":"https://github.com/acme/cordy",
            "ref":"2024",
            "default_branch_hint":"main"
        })
    );

    let generic = Cli::try_parse_from([
        "cordy",
        "project",
        "resource",
        "add",
        "11111111-1111-1111-1111-111111111111",
        "--type",
        "documentation",
        "--ref",
        r#"{"url":"https://docs.example.com"}"#,
    ])
    .expect("generic project resource CLI");
    let Command::Project(ProjectArgs {
        command:
            ProjectCommand::Resource(ProjectResourceArgs {
                command: ProjectResourceCommand::Add(args),
            }),
    }) = &generic.command
    else {
        panic!("expected generic project resource add");
    };
    assert_eq!(
        build_project_resource_add_ref(args).expect("generic ref"),
        serde_json::json!({"url":"https://docs.example.com"})
    );
}

#[tokio::test]
async fn project_resource_list_and_add_use_go_http_and_output_contracts() {
    let project_id = "11111111-1111-1111-1111-111111111111";
    let resource_id = "22222222-2222-2222-2222-222222222222";
    let app = Router::new().route(
            "/api/projects/11111111-1111-1111-1111-111111111111/resources",
            get(move || async move {
                Json(serde_json::json!({"resources":[{
                    "id":resource_id,"resource_type":"github_repo",
                    "resource_ref":{"url":"https://github.com/acme/cordy","ref":"main"},
                    "label":"Cordy"
                }]}))
            })
            .post(|Json(body): Json<Value>| async move {
                assert_eq!(body["resource_type"], "local_directory");
                assert_eq!(body["resource_ref"]["local_path"], "/srv/cordy");
                assert_eq!(body["resource_ref"]["daemon_id"], "daemon-1");
                assert_eq!(body["resource_ref"]["execution_mode"], "worktree");
                Json(serde_json::json!({
                    "id":"33333333-3333-3333-3333-333333333333",
                    "resource_type":"local_directory",
                    "resource_ref":{"local_path":"/srv/cordy","daemon_id":"daemon-1","execution_mode":"worktree"}
                }))
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");
    environment.set("CORDY_TOKEN", "token-1");

    let list = Cli::try_parse_from([
        "cordy", "project", "resource", "list", project_id, "--output", "table",
    ])
    .expect("project resource list CLI");
    let listed = run_with_input(&list, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list project resources");
    assert!(listed.stdout.contains("22222222"));
    assert!(listed
        .stdout
        .contains("https://github.com/acme/cordy @ main"));
    assert!(listed.stdout.contains("Cordy"));

    let add = Cli::try_parse_from([
        "cordy",
        "project",
        "resource",
        "add",
        project_id,
        "--type",
        "local_directory",
        "--local-path",
        "/srv/cordy",
        "--daemon-id",
        "daemon-1",
        "--execution-mode",
        "worktree",
        "--output",
        "table",
    ])
    .expect("project resource add CLI");
    let added = run_with_input(&add, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("add project resource");
    assert!(added
        .stdout
        .contains("33333333-3333-3333-3333-333333333333"));
    assert!(added.stdout.contains("/srv/cordy"));
    task.abort();
}

#[test]
fn project_resource_update_rebuilds_opaque_refs_and_supports_clear_flags() {
    let cli = Cli::try_parse_from([
        "cordy",
        "project",
        "resource",
        "update",
        "11111111-1111-1111-1111-111111111111",
        "2222",
        "--default-branch-hint",
        "trunk",
        "--clear-label",
        "--position",
        "3",
        "--output",
        "table",
    ])
    .expect("project resource update CLI");
    let Command::Project(ProjectArgs {
        command:
            ProjectCommand::Resource(ProjectResourceArgs {
                command: ProjectResourceCommand::Update(args),
            }),
    }) = &cli.command
    else {
        panic!("expected project resource update");
    };
    assert!(args.clear_label);
    assert_eq!(args.position, Some(3));
    let existing = serde_json::json!({
        "url":"https://github.com/acme/cordy",
        "ref":"main",
        "default_branch_hint":"main"
    });
    assert_eq!(
        build_project_resource_update_ref(args, "github_repo", existing.as_object())
            .expect("update ref")
            .expect("changed ref"),
        serde_json::json!({
            "url":"https://github.com/acme/cordy",
            "ref":"main",
            "default_branch_hint":"trunk"
        })
    );
}

#[tokio::test]
async fn project_resource_update_and_remove_use_prefix_put_and_delete_contracts() {
    let project_id = "11111111-1111-1111-1111-111111111111";
    let resource_id = "22222222-2222-2222-2222-222222222222";
    let resource_path =
            "/api/projects/11111111-1111-1111-1111-111111111111/resources/22222222-2222-2222-2222-222222222222";
    let app = Router::new()
            .route(
                "/api/projects/11111111-1111-1111-1111-111111111111/resources",
                get(move || async move {
                    Json(serde_json::json!({"resources":[{
                        "id":resource_id,"resource_type":"github_repo",
                        "resource_ref":{"url":"https://github.com/acme/cordy","ref":"main"},
                        "label":"Cordy"
                    }]}))
                }),
            )
            .route(
                resource_path,
                put(|Json(body): Json<Value>| async move {
                    assert_eq!(body["label"], Value::Null);
                    assert_eq!(body["position"], 3);
                    assert_eq!(
                        body["resource_ref"],
                        serde_json::json!({
                            "url":"https://github.com/acme/cordy",
                            "ref":"main",
                            "default_branch_hint":"trunk"
                        })
                    );
                    Json(serde_json::json!({
                        "id":"22222222-2222-2222-2222-222222222222",
                        "resource_type":"github_repo",
                        "resource_ref":{"url":"https://github.com/acme/cordy","ref":"main","default_branch_hint":"trunk"},
                        "label":""
                    }))
                })
                .delete(|| async { axum::http::StatusCode::NO_CONTENT }),
            );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");
    environment.set("CORDY_TOKEN", "token-1");

    let update = Cli::try_parse_from([
        "cordy",
        "project",
        "resource",
        "update",
        project_id,
        "2222",
        "--default-branch-hint",
        "trunk",
        "--clear-label",
        "--position",
        "3",
        "--output",
        "table",
    ])
    .expect("project resource update CLI");
    let updated = run_with_input(&update, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("update project resource");
    assert!(updated.stdout.contains(resource_id));
    assert!(updated
        .stdout
        .contains("https://github.com/acme/cordy @ main"));

    let remove = Cli::try_parse_from([
        "cordy",
        "project",
        "resource",
        "remove",
        project_id,
        resource_id,
    ])
    .expect("project resource remove CLI");
    let removed = run_with_input(&remove, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("remove project resource");
    assert!(removed.stdout.is_empty());
    assert_eq!(
        removed.stderr,
        format!("Resource {resource_id} removed from project {project_id}.\n")
    );
    task.abort();
}
