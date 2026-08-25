use anyhow::{Context, Result};
use serde_json::Value;

use super::{
    format_metadata_value, format_table, new_api_client, resolve_issue_ref, Cli, Environment,
    IssueUsageArgs, OutputFormat, RunOutput,
};

pub(super) async fn run_issue_usage(
    cli: &Cli,
    environment: &Environment,
    args: &IssueUsageArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let usage: Value = client
        .get_json(&format!("/api/issues/{issue_id}/usage"))
        .await
        .context("get issue usage")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&usage)?),
        OutputFormat::Table => format_table(&[
            vec![
                "INPUT_TOKENS".into(),
                "OUTPUT_TOKENS".into(),
                "CACHE_READ".into(),
                "CACHE_WRITE".into(),
                "RUNS".into(),
            ],
            vec![
                format_metadata_value(usage.get("total_input_tokens")),
                format_metadata_value(usage.get("total_output_tokens")),
                format_metadata_value(usage.get("total_cache_read_tokens")),
                format_metadata_value(usage.get("total_cache_write_tokens")),
                format_metadata_value(usage.get("task_count")),
            ],
        ]),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}
