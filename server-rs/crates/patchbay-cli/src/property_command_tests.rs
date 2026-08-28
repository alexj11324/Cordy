use super::*;
use axum::extract::Request;
use axum::routing::{get, patch, put};
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;
use tokio::net::TcpListener;

#[test]
fn property_read_parser_and_table_match_go_registry_contract() {
    let cli = Cli::try_parse_from([
        "patchbay",
        "property",
        "list",
        "--include-archived",
        "--output",
        "json",
    ])
    .expect("property list CLI");
    let Command::Property(PropertyArgs {
        command: PropertyCommand::List {
            output,
            include_archived,
        },
    }) = &cli.command
    else {
        panic!("expected property list");
    };
    assert_eq!(*output, OutputFormat::Json);
    assert!(*include_archived);

    let properties: Vec<PropertyDefinition> = serde_json::from_value(serde_json::json!([{
        "id":"11111111-1111-1111-1111-111111111111",
        "name":"Severity","type":"select","icon":"shield",
        "config":{"options":[{"id":"option-1","name":"Critical","color":"#ef4444"}]},
        "usage_count":7,"archived":true
    }]))
    .expect("property definitions");
    let table =
        format_property_definitions(&properties, OutputFormat::Table).expect("property table");
    assert!(table.starts_with("ID"));
    assert!(table.contains("11111111-1111-1111-1111-111111111111"));
    assert!(table.contains("shield"));
    assert!(table.contains("Critical"));
    assert!(table.contains("7"));
    assert!(table.contains("yes"));
}

#[tokio::test]
async fn property_list_and_get_preserve_archive_query_and_full_json_fields() {
    let app = Router::new().route(
        "/api/properties",
        get(|request: Request| async move {
            let include_archived = request
                .uri()
                .query()
                .is_some_and(|query| query == "include_archived=true");
            let properties = if include_archived {
                vec![serde_json::json!({
                    "id":"11111111-1111-1111-1111-111111111111",
                    "name":"Severity","type":"select","description":"Impact",
                    "icon":"shield","config":{"options":[{
                        "id":"option-1","name":"Critical","color":"#ef4444"
                    }]},"position":1.5,"archived":true,"usage_count":7,
                    "created_at":"2026-08-24T00:00:00Z","updated_at":"2026-08-24T01:00:00Z"
                })]
            } else {
                Vec::new()
            };
            Json(serde_json::json!({"properties":properties}))
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

    let list = Cli::try_parse_from(["patchbay", "property", "list", "--output", "json"])
        .expect("property list CLI");
    let listed = run_with_input(&list, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list active properties");
    assert_eq!(
        serde_json::from_str::<Value>(&listed.stdout).expect("properties JSON"),
        serde_json::json!([])
    );

    let get = Cli::try_parse_from([
        "patchbay", "property", "get", "severity", "--output", "json",
    ])
    .expect("property get CLI");
    let got = run_with_input(&get, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("get archived property by name");
    let property: Value = serde_json::from_str(&got.stdout).expect("property JSON");
    assert_eq!(property["name"], "Severity");
    assert_eq!(property["description"], "Impact");
    assert_eq!(property["config"]["options"][0]["color"], "#ef4444");
    assert_eq!(property["position"], 1.5);
    assert_eq!(property["usage_count"], 7);
    assert_eq!(property["archived"], true);
    task.abort();
}

#[test]
fn property_mutation_parser_preserves_option_ids_and_clear_values() {
    let cli = Cli::try_parse_from([
        "patchbay",
        "property",
        "update",
        "Severity",
        "--description=",
        "--icon=",
        "--option",
        "critical:#ef4444",
        "--option",
        "Minor",
    ])
    .expect("property update CLI");
    let Command::Property(PropertyArgs {
        command: PropertyCommand::Update(args),
    }) = &cli.command
    else {
        panic!("expected property update");
    };
    assert_eq!(args.description.as_deref(), Some(""));
    assert_eq!(args.icon.as_deref(), Some(""));
    let existing = vec![PropertyOption {
        id: "option-1".into(),
        name: "Critical".into(),
        color: "#000000".into(),
    }];
    assert_eq!(
        parse_property_options(&args.option, &existing),
        vec![
            serde_json::json!({"id":"option-1","name":"critical","color":"#ef4444"}),
            serde_json::json!({"name":"Minor","color":"#6b7280"})
        ]
    );
}

#[tokio::test]
async fn property_create_update_and_archive_use_go_patch_and_output_contracts() {
    let property_id = "11111111-1111-1111-1111-111111111111";
    let definition = move || {
        serde_json::json!({
            "id":property_id,"name":"Severity","type":"select","description":"",
            "icon":"shield","config":{"options":[{
                "id":"option-1","name":"Critical","color":"#ef4444"
            }]},"position":1,"archived":false,"usage_count":0,
            "created_at":"","updated_at":""
        })
    };
    let app = Router::new()
            .route(
                "/api/properties",
                get(move || async move {
                    Json(serde_json::json!({"properties":[definition()]}))
                })
                .post(|Json(body): Json<Value>| async move {
                    assert_eq!(body["name"], "Severity");
                    assert_eq!(body["type"], "select");
                    assert_eq!(body["description"], "");
                    assert_eq!(body["config"]["options"][0]["color"], "#ef4444");
                    Json(serde_json::json!({
                        "id":"11111111-1111-1111-1111-111111111111",
                        "name":"Severity","type":"select","description":"","icon":"shield",
                        "config":{"options":[{"id":"option-1","name":"Critical","color":"#ef4444"}]},
                        "position":1,"archived":false,"usage_count":0,"created_at":"","updated_at":""
                    }))
                }),
            )
            .route(
                "/api/properties/11111111-1111-1111-1111-111111111111",
                patch(|Json(body): Json<Value>| async move {
                    if let Some(archived) = body.get("archived") {
                        return Json(serde_json::json!({
                            "id":"11111111-1111-1111-1111-111111111111",
                            "name":"Severity","type":"select","description":"","icon":"shield",
                            "config":{"options":[]},"position":1,"archived":archived,
                            "usage_count":0,"created_at":"","updated_at":""
                        }));
                    }
                    assert_eq!(body["description"], "Impact");
                    assert_eq!(body["config"]["options"][0]["id"], "option-1");
                    Json(serde_json::json!({
                        "id":"11111111-1111-1111-1111-111111111111",
                        "name":"Severity","type":"select","description":"Impact","icon":"shield",
                        "config":body["config"],"position":1,"archived":false,
                        "usage_count":0,"created_at":"","updated_at":""
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
        "property",
        "create",
        "--name",
        "Severity",
        "--type",
        "select",
        "--icon",
        "shield",
        "--option",
        "Critical:#ef4444",
        "--output",
        "json",
    ])
    .expect("property create CLI");
    let created = run_with_input(&create, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("create property");
    assert_eq!(
        serde_json::from_str::<Value>(&created.stdout).expect("created property")["id"],
        property_id
    );

    let update = Cli::try_parse_from([
        "patchbay",
        "property",
        "update",
        "severity",
        "--description",
        "Impact",
        "--option",
        "Critical:#22c55e",
        "--output",
        "table",
    ])
    .expect("property update CLI");
    let updated = run_with_input(&update, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("update property");
    assert!(updated.stdout.starts_with("Property \"Severity\" updated."));
    assert!(updated.stdout.contains("Critical"));

    let archive = Cli::try_parse_from([
        "patchbay", "property", "archive", "Severity", "--output", "table",
    ])
    .expect("property archive CLI");
    let archived = run_with_input(&archive, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("archive property");
    assert_eq!(archived.stdout, "Property \"Severity\" archived.\n");
    task.abort();
}

#[test]
fn issue_property_parser_resolution_and_rendering_match_go_contract() {
    let cli = Cli::try_parse_from([
        "patchbay",
        "issue",
        "property",
        "set",
        "CORD-18",
        "--name",
        "Platforms",
        "--value=",
        "--output",
        "json",
    ])
    .expect("property set CLI");
    let Command::Issue(IssueArgs {
        command:
            IssueCommand::Property(IssuePropertyArgs {
                command: IssuePropertyCommand::Set(args),
            }),
    }) = &cli.command
    else {
        panic!("expected issue property set");
    };
    assert_eq!(args.value.as_deref(), Some(""));

    let definitions: Vec<PropertyDefinition> = serde_json::from_value(serde_json::json!([
        {
            "id":"property-1","name":"Severity","type":"select","archived":false,
            "config":{"options":[{"id":"option-1","name":"Critical","color":"#f00"}]}
        },
        {
            "id":"property-2","name":"Reviewer","type":"actor","archived":true,
            "config":{"options":[]}
        }
    ]))
    .expect("property definitions");
    assert_eq!(
        resolve_property(&definitions, "severity")
            .expect("case-insensitive name")
            .id,
        "property-1"
    );
    let bag = serde_json::Map::from_iter([
        ("property-1".into(), Value::String("option-1".into())),
        ("property-2".into(), Value::String("member:member-1".into())),
    ]);
    let actors = IssueActorNames(HashMap::from([("member:member-1".into(), "Ada".into())]));
    let rows = build_issue_property_rows(&definitions, &bag, &actors);
    assert_eq!(rows[0].display, "Critical");
    assert_eq!(rows[1].display, "Ada");
    let table = format_issue_property_rows(&rows, OutputFormat::Table).expect("table");
    assert!(table.starts_with("NAME"));
    assert!(table.contains("Severity"));
    assert!(table.contains("Reviewer"));
    let json = format_issue_property_rows(&rows, OutputFormat::Json).expect("JSON");
    let json: Value = serde_json::from_str(&json).expect("rows JSON");
    assert!(json[0].get("archived").is_none());
    assert_eq!(json[1]["archived"], true);
}

#[tokio::test]
async fn issue_property_set_resolves_option_name_and_puts_typed_value() {
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
        )
        .route(
            "/api/properties",
            get(|request: Request| async move {
                assert_eq!(request.uri().query(), Some("include_archived=true"));
                Json(serde_json::json!({
                    "properties":[{
                        "id":"property-1","name":"Severity","type":"select",
                        "config":{"options":[{"id":"option-1","name":"Critical","color":"#f00"}]}
                    }]
                }))
            }),
        )
        .route(
            "/api/issues/issue-uuid/properties/property-1",
            put(|Json(body): Json<Value>| async move {
                assert_eq!(body, serde_json::json!({"value":"option-1"}));
                Json(serde_json::json!({"properties":{"property-1":"option-1"}}))
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
        "patchbay", "issue", "property", "set", "CORD-18", "--name", "severity", "--value",
        "Critical", "--output", "json",
    ])
    .expect("property set CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("set issue property");
    let rows: Value = serde_json::from_str(&output.stdout).expect("property rows JSON");
    assert_eq!(rows[0]["display"], "Critical");
    assert_eq!(rows[0]["value"], "option-1");
    task.abort();
}

#[tokio::test]
async fn issue_property_list_resolves_member_actor_display() {
    let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/properties",
                get(|| async {
                    Json(serde_json::json!({
                        "properties":[{
                            "id":"property-1","name":"Reviewer","type":"actor","config":{}
                        }]
                    }))
                }),
            )
            .route(
                "/api/issues/issue-uuid",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","properties":{"property-1":"member:member-1"}}))
                }),
            )
            .route(
                "/api/workspaces/workspace-1/members",
                get(|| async {
                    Json(vec![serde_json::json!({"user_id":"member-1","name":"Ada"})])
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
        "patchbay", "issue", "property", "list", "CORD-18", "--output", "table",
    ])
    .expect("property list CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("list issue properties");
    assert!(output.stdout.contains("Reviewer"));
    assert!(output.stdout.contains("Ada"));
    task.abort();
}
