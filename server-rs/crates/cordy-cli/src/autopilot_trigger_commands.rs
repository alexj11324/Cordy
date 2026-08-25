use anyhow::{Context, Result};
use serde_json::Value;
use std::time::Duration;

use super::{
    Cli, Environment, OutputFormat, RunOutput, http_timeout, new_api_client, resolve_autopilot_id,
    resolve_current_workspace_id, value_string,
};

pub(super) async fn run_autopilot_trigger(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let timeout = http_timeout(environment.raw("CORDY_HTTP_TIMEOUT"))
        .saturating_add(Duration::from_secs(5))
        .max(Duration::from_secs(30));
    let client = new_api_client(cli, environment)?.with_request_timeout(timeout);
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, _) = resolve_autopilot_id(&client, &workspace_id, id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
    let run: Value = client
        .post_json(
            &format!("/api/autopilots/{autopilot_id}/trigger"),
            &Value::Null,
        )
        .await
        .context("trigger autopilot")?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&run)?),
            OutputFormat::Table => format!(
                "Autopilot triggered: run {} (status: {})\n",
                value_string(&run, "id"),
                value_string(&run, "status")
            ),
        },
        stderr: String::new(),
    })
}
