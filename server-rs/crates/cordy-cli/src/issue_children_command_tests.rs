use super::*;
use axum::extract::Request;
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use std::collections::HashMap;
use std::io::Cursor;
use tokio::net::TcpListener;
#[test]
fn issue_children_parser_supports_alias_output_and_full_id_flag() {
    for name in ["children", "subissues"] {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            name,
            "CORD-18",
            "--output",
            "json",
            "--full-id",
        ])
        .expect("children CLI");
        match cli.command {
            Command::Issue(IssueArgs {
                command:
                    IssueCommand::Children {
                        id,
                        output,
                        full_id,
                    },
            }) => {
                assert_eq!(id, "CORD-18");
                assert_eq!(output, OutputFormat::Json);
                assert!(full_id);
            }
            _ => panic!("expected issue children"),
        }
    }
}

#[test]
fn issue_children_sort_group_and_terminal_count_match_go() {
    let mut children = vec![
        serde_json::json!({"id":"u1","identifier":"CORD-4","stage":null,"status":"todo"}),
        serde_json::json!({"id":"s2a","identifier":"CORD-2","stage":2,"status":"cancelled","status_category":"cancelled"}),
        serde_json::json!({"id":"s1a","identifier":"CORD-1","stage":1,"status":"gate_approved","status_category":"done"}),
        serde_json::json!({"id":"s2b","identifier":"CORD-3","stage":2,"status":"in_progress","status_category":"in_progress"}),
        serde_json::json!({"id":"u2","identifier":"CORD-5","status":"done"}),
    ];
    children.sort_by_key(|child| child_stage(child).map_or((true, 0), |stage| (false, stage)));
    let identifiers = children
        .iter()
        .map(|child| value_string(child, "identifier"))
        .collect::<Vec<_>>();
    assert_eq!(
        identifiers,
        vec![
            String::from("CORD-1"),
            String::from("CORD-2"),
            String::from("CORD-3"),
            String::from("CORD-4"),
            String::from("CORD-5"),
        ]
    );
    let grouped = serde_json::to_value(group_issue_children(&children)).expect("group JSON");
    assert_eq!(grouped["total"], 5);
    assert_eq!(grouped["stages"][0]["stage"], 1);
    assert_eq!(grouped["stages"][0]["total"], 1);
    assert_eq!(grouped["stages"][0]["done"], 1);
    assert_eq!(grouped["stages"][1]["stage"], 2);
    assert_eq!(grouped["stages"][1]["total"], 2);
    assert_eq!(grouped["stages"][1]["done"], 1);
    assert_eq!(grouped["unstaged"].as_array().map(Vec::len), Some(2));
}

#[tokio::test]
async fn issue_children_resolves_parent_and_fetches_children_endpoint() {
    let app = Router::new()
        .route(
            "/api/issues/CORD-18",
            get(|| async {
                Json(serde_json::json!({
                    "id": "11111111-1111-1111-1111-111111111111",
                    "identifier": "CORD-18"
                }))
            }),
        )
        .route(
            "/api/issues/11111111-1111-1111-1111-111111111111/children",
            get(|request: Request| async move {
                assert_eq!(request.headers()["authorization"], "Bearer token-1");
                Json(serde_json::json!({
                    "issues": [
                        {"id":"child-2","identifier":"CORD-20","stage":2,"status":"todo"},
                        {"id":"child-1","identifier":"CORD-19","stage":1,"status":"done"}
                    ]
                }))
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("http://{address}"));
    environment.set("CORDY_WORKSPACE_ID", "workspace-1");
    environment.set("CORDY_TOKEN", "token-1");
    let cli = Cli::try_parse_from(["cordy", "issue", "children", "CORD-18", "--output", "json"])
        .expect("children CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("children");
    let grouped: Value = serde_json::from_str(&output.stdout).expect("children JSON");
    assert_eq!(grouped["stages"][0]["stage"], 1);
    assert_eq!(grouped["stages"][1]["stage"], 2);
    assert_eq!(grouped["stages"][0]["done"], 1);
    task.abort();
}

#[test]
fn issue_children_table_renders_stage_key_and_actor() {
    let children = vec![serde_json::json!({
        "id": "child-1",
        "identifier": "CORD-19",
        "stage": 1,
        "title": "First barrier",
        "status": "in_progress",
        "priority": "high",
        "assignee_type": "agent",
        "assignee_id": "agent-1"
    })];
    let actors = IssueActorNames(HashMap::from([("agent:agent-1".into(), "CordyBot".into())]));
    let table = format_issue_children_table(&children, &actors);
    assert!(table.starts_with("STAGE"));
    assert!(table.contains("CORD-19"));
    assert!(table.contains("First barrier"));
    assert!(table.contains("agent:CordyBot"));
}
