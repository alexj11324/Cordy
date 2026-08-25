use super::*;
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use std::collections::HashMap;
use std::io::Cursor;
use tokio::net::TcpListener;

#[test]
fn issue_timeline_parser_filter_and_table_match_go_contract() {
    let cli = Cli::try_parse_from([
        "cordy",
        "issue",
        "history",
        "CORD-18",
        "--action",
        "status_changed,priority_changed",
        "--since",
        "2026-08-19T00:00:00Z",
        "--tail",
        "1",
        "--full-id",
    ])
    .expect("timeline CLI alias");
    let Command::Issue(IssueArgs {
        command: IssueCommand::Timeline(args),
    }) = &cli.command
    else {
        panic!("expected issue timeline");
    };
    let filter = build_timeline_filter(args).expect("timeline filter");
    assert!(filter.activity_only);
    assert!(filter.actions.contains("status_changed"));
    assert_eq!(filter.tail, 1);
    let entries = filter_timeline(
        vec![
            serde_json::json!({
                "type":"comment","created_at":"2026-08-20T00:00:00Z","content":"ignored"
            }),
            serde_json::json!({
                "type":"activity","action":"status_changed",
                "created_at":"2026-08-20T00:00:00Z","details":{"from":"todo","to":"done"}
            }),
            serde_json::json!({
                "type":"activity","action":"priority_changed",
                "created_at":"2026-08-21T00:00:00Z","details":{"from":"low","to":"high"}
            }),
        ],
        &filter,
    );
    assert_eq!(entries.len(), 1);
    assert_eq!(value_string(&entries[0], "action"), "priority_changed");

    let actors = IssueActorNames(HashMap::from([("member:member-1".into(), "Ada".into())]));
    let table = format_issue_timeline_table(
        &[
            serde_json::json!({
                "type":"activity","action":"assignee_changed",
                "actor_type":"member","actor_id":"member-1",
                "created_at":"2026-08-24T12:34:56Z",
                "details":{"from_type":"member","from_id":"old-member","to_type":"member","to_id":"member-1"}
            }),
            serde_json::json!({
                "type":"comment","actor_type":"system","actor_id":null,
                "created_at":"2026-08-24T13:00:00Z",
                "content":"multi\nline   comment"
            }),
        ],
        &actors,
        false,
    );
    assert!(table.starts_with("TIME"));
    assert!(table.contains("member:Ada"));
    assert!(table.contains("member:old-memb → member:Ada"));
    assert!(table.contains("multi line comment"));
    assert!(table.contains("system"));
}

#[test]
fn issue_timeline_rejects_invalid_since_and_negative_tail() {
    let invalid_since = Cli::try_parse_from([
        "cordy",
        "issue",
        "timeline",
        "CORD-18",
        "--since",
        "yesterday",
    ])
    .expect("invalid since parses");
    let Command::Issue(IssueArgs {
        command: IssueCommand::Timeline(args),
    }) = &invalid_since.command
    else {
        panic!("expected timeline");
    };
    assert!(build_timeline_filter(args)
        .expect_err("invalid since")
        .to_string()
        .contains("expected RFC3339"));

    let negative_tail =
        Cli::try_parse_from(["cordy", "issue", "timeline", "CORD-18", "--tail", "-1"])
            .expect("negative tail parses");
    let Command::Issue(IssueArgs {
        command: IssueCommand::Timeline(args),
    }) = &negative_tail.command
    else {
        panic!("expected timeline");
    };
    assert_eq!(
        build_timeline_filter(args)
            .expect_err("negative tail")
            .to_string(),
        "--tail must be >= 0"
    );
}

#[tokio::test]
async fn issue_timeline_filters_json_and_surfaces_truncation_header() {
    let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/issue-uuid/timeline",
                get(|| async {
                    let mut headers = HeaderMap::new();
                    headers.insert(
                        "X-Timeline-Truncated",
                        "activity,comment".parse().expect("truncation header"),
                    );
                    (
                        headers,
                        Json(vec![
                            serde_json::json!({
                                "type":"comment","created_at":"2026-08-20T00:00:00Z","content":"note"
                            }),
                            serde_json::json!({
                                "type":"activity","action":"status_changed",
                                "created_at":"2026-08-21T00:00:00Z","details":{"from":"todo","to":"done"}
                            }),
                        ]),
                    )
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
    let cli = Cli::try_parse_from([
        "cordy",
        "issue",
        "timeline",
        "CORD-18",
        "--activity-only",
        "--output",
        "json",
    ])
    .expect("timeline CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("timeline");
    let entries: Value = serde_json::from_str(&output.stdout).expect("timeline JSON");
    assert_eq!(entries.as_array().expect("entries").len(), 1);
    assert_eq!(entries[0]["action"], "status_changed");
    assert!(output.stderr.contains("activity,comment"));
    assert!(output.stderr.contains("older entries are missing"));
    task.abort();
}
