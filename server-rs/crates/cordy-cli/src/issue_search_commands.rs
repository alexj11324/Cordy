use anyhow::{Context, Result};
use serde_json::Value;
use url::form_urlencoded;

use super::{
    format_table, new_api_client, value_string, Cli, Environment, IssueSearchArgs, OutputFormat,
    RunOutput,
};

pub(super) async fn run_issue_search(
    cli: &Cli,
    environment: &Environment,
    args: &IssueSearchArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("q", &args.query);
    if args.limit > 0 {
        serializer.append_pair("limit", &args.limit.to_string());
    }
    if args.include_closed {
        serializer.append_pair("include_closed", "true");
    }
    let result: Value = client
        .get_json(&format!("/api/issues/search?{}", serializer.finish()))
        .await
        .context("search issues")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
        OutputFormat::Table => {
            let issues = result
                .get("issues")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            format_issue_search_table(issues)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) fn format_issue_search_table(issues: &[Value]) -> String {
    let mut rows = vec![vec![
        "KEY".into(),
        "TITLE".into(),
        "STATUS".into(),
        "MATCH".into(),
    ]];
    for issue in issues {
        let mut match_info = value_string(issue, "match_source");
        let snippet = value_string(issue, "matched_snippet");
        if !snippet.is_empty() {
            let snippet = if snippet.chars().count() > 50 {
                format!("{}...", snippet.chars().take(47).collect::<String>())
            } else {
                snippet
            };
            match_info.push_str(": ");
            match_info.push_str(&snippet);
        }
        rows.push(vec![
            value_string(issue, "identifier"),
            value_string(issue, "title"),
            value_string(issue, "status"),
            match_info,
        ]);
    }
    format_table(&rows)
}
