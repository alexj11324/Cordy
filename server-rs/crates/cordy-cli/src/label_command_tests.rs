use super::*;
use axum::routing::{post, put};
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use tokio::net::TcpListener;

#[test]
fn label_parser_and_tables_match_go_registry_contract() {
    let create = Cli::try_parse_from([
        "cordy", "label", "create", "--name", "Bug", "--color", "#ff0000", "--output", "table",
    ])
    .expect("label create CLI");
    let Command::Label(LabelArgs {
        command: LabelCommand::Create(args),
    }) = &create.command
    else {
        panic!("expected label create");
    };
    assert_eq!(args.name.as_deref(), Some("Bug"));
    assert_eq!(args.color.as_deref(), Some("#ff0000"));
    assert_eq!(args.output, OutputFormat::Table);

    let label = serde_json::json!({
        "id":"11111111-1111-1111-1111-111111111111","name":"Bug","color":"#ff0000",
        "created_at":"2026-08-24T12:34:56Z"
    });
    let short = format_workspace_label_table(std::slice::from_ref(&label), false);
    assert!(short.starts_with("ID"));
    assert!(short.contains("11111111"));
    assert!(short.contains("2026-08-24"));
    let details = format_label_result(&label, OutputFormat::Table, true).expect("details");
    assert!(details.contains("11111111-1111-1111-1111-111111111111"));
}

#[tokio::test]
async fn label_create_update_and_delete_use_go_http_and_output_contracts() {
    let label_id = "11111111-1111-1111-1111-111111111111";
    let app = Router::new()
        .route(
            "/api/labels",
            post(|Json(body): Json<Value>| async move {
                assert_eq!(body, serde_json::json!({"name":"Bug","color":"#ff0000"}));
                Json(serde_json::json!({
                    "id":"11111111-1111-1111-1111-111111111111",
                    "name":"Bug","color":"#ff0000"
                }))
            }),
        )
        .route(
            "/api/labels/11111111-1111-1111-1111-111111111111",
            put(|Json(body): Json<Value>| async move {
                assert_eq!(body, serde_json::json!({"name":"Defect"}));
                Json(serde_json::json!({
                    "id":"11111111-1111-1111-1111-111111111111",
                    "name":"Defect","color":"#ff0000"
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

    let create = Cli::try_parse_from([
        "cordy", "label", "create", "--name", "Bug", "--color", "#ff0000",
    ])
    .expect("label create CLI");
    let created = run_with_input(&create, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("create label");
    assert_eq!(
        serde_json::from_str::<Value>(&created.stdout).expect("created JSON")["name"],
        "Bug"
    );

    let update = Cli::try_parse_from([
        "cordy", "label", "update", label_id, "--name", "Defect", "--output", "table",
    ])
    .expect("label update CLI");
    let updated = run_with_input(&update, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("update label");
    assert!(updated.stdout.contains("Defect"));

    let delete = Cli::try_parse_from(["cordy", "label", "delete", label_id, "--output", "json"])
        .expect("label delete CLI");
    let deleted = run_with_input(&delete, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("delete label");
    let deleted: Value = serde_json::from_str(&deleted.stdout).expect("deleted JSON");
    assert_eq!(deleted["id"], label_id);
    assert_eq!(deleted["deleted"], true);
    task.abort();
}
