use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::time::{Duration, Instant};

use super::runtime_update_output::format_runtime_update_result;
use super::{
    http_timeout, new_api_client, value_string, Cli, Environment, OutputFormat, RunOutput,
};

pub(super) async fn run_runtime_update(
    cli: &Cli,
    environment: &Environment,
    runtime_id: &str,
    target_version: Option<&str>,
    output: OutputFormat,
    wait: bool,
) -> Result<RunOutput> {
    run_runtime_update_with_policy(
        cli,
        environment,
        runtime_id,
        target_version,
        output,
        wait,
        Duration::from_secs(2),
        Duration::from_secs(150),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_runtime_update_with_policy(
    cli: &Cli,
    environment: &Environment,
    runtime_id: &str,
    target_version: Option<&str>,
    output: OutputFormat,
    wait: bool,
    poll_interval: Duration,
    max_wait: Duration,
) -> Result<RunOutput> {
    let request_timeout = http_timeout(environment.raw("CORDY_HTTP_TIMEOUT")).max(max_wait);
    let client = new_api_client(cli, environment)?.with_request_timeout(request_timeout);
    let target_version = target_version
        .filter(|version| !version.is_empty())
        .context("--target-version is required")?;
    let started = Instant::now();
    let mut update: Value = client
        .post_json(
            &format!("/api/runtimes/{runtime_id}/update"),
            &serde_json::json!({"target_version":target_version}),
        )
        .await
        .context("initiate update")?;
    if !wait {
        return format_runtime_update_result(&update, output, false);
    }
    let update_id = value_string(&update, "id");
    let remaining = max_wait.saturating_sub(started.elapsed());
    let poll = async {
        loop {
            tokio::time::sleep(poll_interval).await;
            update = client
                .get_json(&format!("/api/runtimes/{runtime_id}/update/{update_id}"))
                .await
                .context("get update status")?;
            if matches!(
                value_string(&update, "status").as_str(),
                "completed" | "failed" | "timeout"
            ) {
                return Ok::<Value, anyhow::Error>(update.clone());
            }
        }
    };
    match tokio::time::timeout(remaining, poll).await {
        Ok(Ok(final_update)) => format_runtime_update_result(&final_update, output, true),
        Ok(Err(error)) => Err(error),
        Err(_) => bail!(
            "timed out waiting for update (last status: {})",
            value_string(&update, "status")
        ),
    }
}
