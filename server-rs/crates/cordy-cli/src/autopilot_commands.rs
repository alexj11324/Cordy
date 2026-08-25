use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::fmt::Write;
use std::io::{Read, Write as IoWrite};
use std::time::Duration;

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

pub(super) async fn run_autopilot_trigger_add(
    cli: &Cli,
    environment: &Environment,
    args: &AutopilotTriggerAddArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let kind = if args.kind.is_empty() {
        "schedule"
    } else {
        args.kind.as_str()
    };
    if !matches!(kind, "schedule" | "webhook") {
        bail!("--kind must be schedule or webhook");
    }
    if kind == "schedule" && args.cron.is_empty() {
        bail!("--cron is required for --kind schedule");
    }
    if kind == "webhook" && !args.timezone.is_empty() {
        bail!("--timezone is only valid with --kind schedule");
    }
    if kind == "webhook" && !args.cron.is_empty() {
        bail!("--cron is only valid with --kind schedule");
    }

    let mut body = serde_json::Map::from_iter([("kind".into(), Value::String(kind.into()))]);
    if kind == "schedule" {
        body.insert("cron_expression".into(), Value::String(args.cron.clone()));
        if !args.timezone.is_empty() {
            body.insert("timezone".into(), Value::String(args.timezone.clone()));
        }
    }
    if !args.label.is_empty() {
        body.insert("label".into(), Value::String(args.label.clone()));
    }
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, _) = resolve_autopilot_id(&client, &workspace_id, &args.autopilot_id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
    let result: Value = client
        .post_json(&format!("/api/autopilots/{autopilot_id}/triggers"), &body)
        .await
        .context("create trigger")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
        OutputFormat::Table => {
            let mut text = format!(
                "Trigger created: {} (kind={})\n",
                value_string(&result, "id"),
                value_string(&result, "kind")
            );
            if kind == "webhook" {
                if let Some(url) = autopilot_webhook_url(&result, client.base_url()) {
                    let _ = writeln!(text, "Webhook URL: {url}");
                }
            }
            text
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) async fn run_autopilot_trigger_update(
    cli: &Cli,
    environment: &Environment,
    args: &AutopilotTriggerUpdateArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let mut body = serde_json::Map::new();
    if let Some(enabled) = args.enabled {
        body.insert("enabled".into(), Value::Bool(enabled));
    }
    for (key, value) in [
        ("cron_expression", args.cron.as_ref()),
        ("timezone", args.timezone.as_ref()),
        ("label", args.label.as_ref()),
    ] {
        if let Some(value) = value {
            body.insert(key.into(), Value::String(value.clone()));
        }
    }
    if body.is_empty() {
        bail!("no fields to update; use --enabled, --cron, --timezone, or --label");
    }
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, _) = resolve_autopilot_id(&client, &workspace_id, &args.autopilot_id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
    let trigger_id = resolve_autopilot_trigger_id(&client, &autopilot_id, &args.trigger_id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve trigger: {error:#}"))?;
    let result: Value = client
        .patch_json(
            &format!("/api/autopilots/{autopilot_id}/triggers/{trigger_id}"),
            &body,
        )
        .await
        .context("update trigger")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
            OutputFormat::Table => {
                format!("Trigger updated: {}\n", value_string(&result, "id"))
            }
        },
        stderr: String::new(),
    })
}

pub(super) async fn run_autopilot_trigger_delete(
    cli: &Cli,
    environment: &Environment,
    autopilot: &str,
    trigger: &str,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, _) = resolve_autopilot_id(&client, &workspace_id, autopilot)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
    let trigger_id = resolve_autopilot_trigger_id(&client, &autopilot_id, trigger)
        .await
        .map_err(|error| anyhow::anyhow!("resolve trigger: {error:#}"))?;
    client
        .delete(&format!(
            "/api/autopilots/{autopilot_id}/triggers/{trigger_id}"
        ))
        .await
        .context("delete trigger")?;
    Ok(RunOutput {
        stdout: format!("Trigger {trigger_id} deleted.\n"),
        stderr: String::new(),
    })
}

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
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
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
