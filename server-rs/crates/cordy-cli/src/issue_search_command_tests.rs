use super::*;
use axum::extract::Request;
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use std::io::Cursor;

#[test]
fn issue_search_parser_and_table_match_go_contract() {
    let cli = Cli::try_parse_from([
        "cordy",
        "issue",
        "search",
        "cache bug",
        "--limit",
        "5",
        "--include-closed",
        "--output",
        "json",
    ])
    .expect("search CLI");
    let args = issue_search_args(&cli);
    assert_eq!(args.query, "cache bug");
    assert_eq!(args.limit, 5);
    assert!(args.include_closed);
    assert_eq!(args.output, OutputFormat::Json);

    let table = format_issue_search_table(&[serde_json::json!({
        "identifier":"CORD-18","title":"Cache issue","status":"todo",
        "match_source":"comment","matched_snippet":"x".repeat(51)
    })]);
    assert!(table.starts_with("KEY"));
    assert!(table.contains("CORD-18"));
    assert!(table.contains("comment: "));
    assert!(table.contains("xxx..."));
}

#[tokio::test]
async fn issue_search_encodes_query_and_preserves_json_envelope() {
    let app = Router::new().route(
        "/api/issues/search",
        get(|request: Request| async move {
            let query = request.uri().query().unwrap_or_default();
            assert!(query.contains("q=cache+bug"));
            assert!(query.contains("limit=5"));
            assert!(query.contains("include_closed=true"));
            Json(serde_json::json!({
                "issues":[{"id":"issue-1","identifier":"CORD-18","title":"Cache bug"}],
                "total":1
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
    let cli = Cli::try_parse_from([
        "cordy",
        "issue",
        "search",
        "cache bug",
        "--limit",
        "5",
        "--include-closed",
        "--output",
        "json",
    ])
    .expect("search CLI");
    let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("search issues");
    let result: Value = serde_json::from_str(&output.stdout).expect("search JSON");
    assert_eq!(result["total"], 1);
    assert_eq!(result["issues"][0]["identifier"], "CORD-18");
    task.abort();
}
