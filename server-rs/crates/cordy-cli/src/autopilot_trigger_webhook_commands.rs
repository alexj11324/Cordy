use anyhow::{Context, Result};
use serde_json::Value;
use std::fmt::Write;
use std::io::{Read, Write as IoWrite};

use super::{
    autopilot_webhook_url, context_autopilot_resolution, new_api_client, read_setup_confirmation,
    resolve_autopilot_id, resolve_autopilot_trigger_id, resolve_current_workspace_id, value_string,
    AutopilotTriggerRotateUrlArgs, Cli, Environment, OutputFormat, RunOutput,
};

pub(super) async fn run_autopilot_trigger_rotate_url<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &AutopilotTriggerRotateUrlArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, _) = resolve_autopilot_id(&client, &workspace_id, &args.autopilot_id)
        .await
        .map_err(context_autopilot_resolution)?;
    let trigger_id = resolve_autopilot_trigger_id(&client, &autopilot_id, &args.trigger_id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve trigger: {error:#}"))?;

    if !args.yes && !confirm_webhook_rotation(input)? {
        return Ok(RunOutput {
            stdout: String::new(),
            stderr: String::new(),
        });
    }

    let result: Value = client
        .post_json(
            &format!("/api/autopilots/{autopilot_id}/triggers/{trigger_id}/rotate-webhook-token"),
            &Value::Null,
        )
        .await
        .context("rotate webhook url")?;

    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
        OutputFormat::Table => {
            let mut text = format!(
                "Webhook URL rotated for trigger {}\n",
                value_string(&result, "id")
            );
            if let Some(url) = autopilot_webhook_url(&result, client.base_url()) {
                let _ = writeln!(text, "Webhook URL: {url}");
            }
            text
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

fn confirm_webhook_rotation<R: Read>(input: &mut R) -> Result<bool> {
    const PROMPT: &str =
        "This will invalidate the current webhook URL immediately. Continue? [y/N] \n";
    let mut stderr = std::io::stderr();
    stderr
        .write_all(PROMPT.as_bytes())
        .context("write webhook rotation prompt")?;
    stderr.flush().context("flush webhook rotation prompt")?;
    let answer = read_setup_confirmation(input)?;
    if matches!(answer.as_str(), "y" | "yes") {
        return Ok(true);
    }
    stderr
        .write_all(b"Aborted.\n")
        .context("write webhook rotation abort")?;
    stderr.flush().context("flush webhook rotation abort")?;
    Ok(false)
}
