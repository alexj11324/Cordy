use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::Value;

use super::{
    format_table, new_api_client, resolve_issue_ref, value_string, Cli, Environment,
    IssuePullRequestAttachArgs, OutputFormat, RunOutput,
};

pub(super) async fn run_issue_pull_requests(
    cli: &Cli,
    environment: &Environment,
    input: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, input)
        .await
        .context("resolve issue")?;
    let result: Value = client
        .get_json(&format!("/api/issues/{issue_id}/pull-requests"))
        .await
        .context("list issue pull requests")?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
        OutputFormat::Table => format_issue_pull_requests_table(&result),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) fn format_issue_pull_requests_table(result: &Value) -> String {
    let pull_requests = result
        .get("pull_requests")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut rows = Vec::with_capacity(pull_requests.len() + 1);
    rows.push(vec![
        "NUMBER".into(),
        "STATE".into(),
        "TITLE".into(),
        "URL".into(),
    ]);
    rows.extend(pull_requests.iter().map(|pull_request| {
        let url = match value_string(pull_request, "url") {
            value if value.is_empty() => value_string(pull_request, "html_url"),
            value => value,
        };
        vec![
            value_string(pull_request, "number"),
            value_string(pull_request, "state"),
            value_string(pull_request, "title"),
            url,
        ]
    }));
    format_table(&rows)
}

#[derive(Debug, Serialize)]
struct AttachPullRequestBody {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    head_sha: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    close_intent: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

pub(super) async fn run_issue_pull_request_attach(
    cli: &Cli,
    environment: &Environment,
    args: &IssuePullRequestAttachArgs,
) -> Result<RunOutput> {
    let url = args.url.trim();
    if url.is_empty() {
        bail!("--url is required (https://github.com/{{owner}}/{{repo}}/pull/{{number}})");
    }
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let optional = |value: &Option<String>| {
        value
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .cloned()
    };
    let body = AttachPullRequestBody {
        url: url.into(),
        title: optional(&args.title),
        state: optional(&args.state),
        branch: optional(&args.branch),
        head_sha: optional(&args.head_sha),
        close_intent: args.close_intent,
    };
    let result: Value = client
        .post_json(&format!("/api/issues/{issue_id}/pull-requests"), &body)
        .await
        .context("attach pull request")?;
    let wrapped = serde_json::json!({
        "pull_request": result.get("pull_request").cloned().unwrap_or(Value::Null)
    });
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&wrapped)?),
        OutputFormat::Table => format_issue_pull_requests_table(&serde_json::json!({
            "pull_requests": [wrapped["pull_request"].clone()]
        })),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}
