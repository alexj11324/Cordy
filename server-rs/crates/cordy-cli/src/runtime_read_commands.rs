use anyhow::{Context, Result};
use serde_json::Value;

use super::{format_runtime_rows, new_api_client, Cli, Environment, OutputFormat, RunOutput};

pub(super) async fn run_runtime_list(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let runtimes: Vec<Value> = client
        .get_json("/api/runtimes")
        .await
        .context("list runtimes")?;
    Ok(RunOutput {
        stdout: format_runtime_rows(
            &runtimes,
            output,
            &["ID", "NAME", "MODE", "PROVIDER", "STATUS", "LAST_SEEN"],
            &[
                "id",
                "name",
                "runtime_mode",
                "provider",
                "status",
                "last_seen_at",
            ],
        )?,
        stderr: String::new(),
    })
}

pub(super) async fn run_runtime_usage(
    cli: &Cli,
    environment: &Environment,
    runtime_id: &str,
    output: OutputFormat,
    days: i32,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    if !(1..=365).contains(&days) {
        anyhow::bail!("--days must be between 1 and 365");
    }
    let usage: Vec<Value> = client
        .get_json(&format!("/api/runtimes/{runtime_id}/usage?days={days}"))
        .await
        .context("get runtime usage")?;
    Ok(RunOutput {
        stdout: format_runtime_rows(
            &usage,
            output,
            &[
                "DATE",
                "PROVIDER",
                "MODEL",
                "INPUT_TOKENS",
                "OUTPUT_TOKENS",
                "CACHE_READ",
                "CACHE_WRITE",
            ],
            &[
                "date",
                "provider",
                "model",
                "input_tokens",
                "output_tokens",
                "cache_read_tokens",
                "cache_write_tokens",
            ],
        )?,
        stderr: String::new(),
    })
}

pub(super) async fn run_runtime_activity(
    cli: &Cli,
    environment: &Environment,
    runtime_id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let activity: Vec<Value> = client
        .get_json(&format!("/api/runtimes/{runtime_id}/activity"))
        .await
        .context("get runtime activity")?;
    Ok(RunOutput {
        stdout: format_runtime_rows(&activity, output, &["HOUR", "COUNT"], &["hour", "count"])?,
        stderr: String::new(),
    })
}
