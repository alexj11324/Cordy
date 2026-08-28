use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Write;
use std::io::{Read, Write as IoWrite};
use std::time::Duration;
use url::form_urlencoded;

use super::{
    autopilot_webhook_url, format_autopilot_runs_table, format_autopilot_table, http_timeout,
    load_autopilot_agent_names, new_api_client, read_setup_confirmation, required_workspace_id,
    resolve_autopilot_agent, resolve_autopilot_id, resolve_autopilot_subscribers,
    resolve_autopilot_trigger_id, resolve_current_workspace_id, resolve_project_reference,
    value_string, Cli, Environment, OutputFormat, RunOutput,
};

#[derive(Debug, Deserialize, Serialize)]
struct AutopilotListEnvelope {
    autopilots: Vec<Value>,
    total: i64,
}

pub(super) async fn run_autopilot_list(
    cli: &Cli,
    environment: &Environment,
    status: &str,
    output: OutputFormat,
    full_id: bool,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = required_workspace_id(cli, environment)?;
    let path = if status.is_empty() {
        "/api/autopilots".into()
    } else {
        format!(
            "/api/autopilots?status={}",
            form_urlencoded::byte_serialize(status.as_bytes()).collect::<String>()
        )
    };
    let response: AutopilotListEnvelope =
        client.get_json(&path).await.context("list autopilots")?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&response)?),
        OutputFormat::Table => {
            let agents =
                load_autopilot_agent_names(&client, &workspace_id, &response.autopilots).await;
            format_autopilot_table(&response.autopilots, full_id, &agents)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) async fn run_autopilot_get(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, _) = resolve_autopilot_id(&client, &workspace_id, id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
    let response: Value = client
        .get_json(&format!("/api/autopilots/{autopilot_id}"))
        .await
        .context("get autopilot")?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&response)?),
        OutputFormat::Table => {
            let autopilot = response.get("autopilot").unwrap_or(&Value::Null);
            let agents =
                load_autopilot_agent_names(&client, &workspace_id, std::slice::from_ref(autopilot))
                    .await;
            format_autopilot_table(std::slice::from_ref(autopilot), true, &agents)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}
pub(super) async fn run_autopilot_create(
    cli: &Cli,
    environment: &Environment,
    args: &AutopilotCreateArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = required_workspace_id(cli, environment)?;
    let title = args.title.as_deref().unwrap_or_default();
    if title.is_empty() {
        bail!("--title is required");
    }
    let agent = args.agent.as_deref().unwrap_or_default();
    if agent.is_empty() {
        bail!("--agent is required (agent name or ID)");
    }
    let mode = args.mode.as_deref().unwrap_or_default();
    if mode.is_empty() {
        bail!("--mode is required (create_issue or run_only)");
    }
    if !matches!(mode, "create_issue" | "run_only") {
        bail!("--mode must be create_issue or run_only");
    }

    let agent_id = resolve_autopilot_agent(&client, &workspace_id, agent)
        .await
        .map_err(|error| anyhow::anyhow!("resolve agent: {error:#}"))?;
    let mut body = serde_json::Map::from_iter([
        ("title".into(), Value::String(title.into())),
        ("assignee_id".into(), Value::String(agent_id)),
        ("execution_mode".into(), Value::String(mode.into())),
    ]);
    if !args.description.is_empty() {
        body.insert(
            "description".into(),
            Value::String(args.description.clone()),
        );
    }
    if let Some(priority) = &args.priority {
        body.insert("priority".into(), Value::String(priority.clone()));
    }
    if !args.project.is_empty() {
        let project_id = resolve_project_reference(&client, &workspace_id, &args.project)
            .await
            .map(|(id, _)| id)
            .map_err(|error| anyhow::anyhow!("resolve project: {error:#}"))?;
        body.insert("project_id".into(), Value::String(project_id));
    }
    if !args.issue_title_template.is_empty() {
        body.insert(
            "issue_title_template".into(),
            Value::String(args.issue_title_template.clone()),
        );
    }
    if !args.subscriber.is_empty() {
        body.insert(
            "subscribers".into(),
            Value::Array(
                resolve_autopilot_subscribers(&client, &workspace_id, &args.subscriber).await?,
            ),
        );
    }

    let result: Value = client
        .post_json("/api/autopilots", &body)
        .await
        .context("create autopilot")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
            OutputFormat::Table => format!(
                "Autopilot created: {} ({})\n",
                value_string(&result, "title"),
                value_string(&result, "id")
            ),
        },
        stderr: String::new(),
    })
}

pub(super) async fn run_autopilot_update(
    cli: &Cli,
    environment: &Environment,
    args: &AutopilotUpdateArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, _) = resolve_autopilot_id(&client, &workspace_id, &args.id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;

    let mut body = serde_json::Map::new();
    for (key, value) in [
        ("title", args.title.as_ref()),
        ("description", args.description.as_ref()),
        ("priority", args.priority.as_ref()),
        ("status", args.status.as_ref()),
        ("issue_title_template", args.issue_title_template.as_ref()),
    ] {
        if let Some(value) = value {
            body.insert(key.into(), Value::String(value.clone()));
        }
    }
    if let Some(agent) = &args.agent {
        let agent_id = resolve_autopilot_agent(&client, &workspace_id, agent)
            .await
            .map_err(|error| anyhow::anyhow!("resolve agent: {error:#}"))?;
        body.insert("assignee_type".into(), Value::String("agent".into()));
        body.insert("assignee_id".into(), Value::String(agent_id));
    }
    if let Some(project) = &args.project {
        let value = if project.is_empty() {
            Value::Null
        } else {
            let id = resolve_project_reference(&client, &workspace_id, project)
                .await
                .map(|(id, _)| id)
                .map_err(|error| anyhow::anyhow!("resolve project: {error:#}"))?;
            Value::String(id)
        };
        body.insert("project_id".into(), value);
    }
    if let Some(mode) = &args.mode {
        if !matches!(mode.as_str(), "create_issue" | "run_only") {
            bail!("--mode must be create_issue or run_only");
        }
        body.insert("execution_mode".into(), Value::String(mode.clone()));
    }
    if args.clear_subscribers && !args.subscriber.is_empty() {
        bail!("--subscriber and --clear-subscribers are mutually exclusive");
    }
    if args.clear_subscribers {
        body.insert("subscribers".into(), Value::Array(Vec::new()));
    } else if !args.subscriber.is_empty() {
        body.insert(
            "subscribers".into(),
            Value::Array(
                resolve_autopilot_subscribers(&client, &workspace_id, &args.subscriber).await?,
            ),
        );
    }
    if body.is_empty() {
        bail!(
            "no fields to update; use flags like --title, --description, --agent, --status, --mode, etc."
        );
    }

    let result: Value = client
        .patch_json(&format!("/api/autopilots/{autopilot_id}"), &body)
        .await
        .context("update autopilot")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
            OutputFormat::Table => format!(
                "Autopilot updated: {} ({})\n",
                value_string(&result, "title"),
                value_string(&result, "id")
            ),
        },
        stderr: String::new(),
    })
}
pub(super) async fn run_autopilot_delete(
    cli: &Cli,
    environment: &Environment,
    id: &str,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, display) = resolve_autopilot_id(&client, &workspace_id, id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
    client
        .delete(&format!("/api/autopilots/{autopilot_id}"))
        .await
        .context("delete autopilot")?;
    Ok(RunOutput {
        stdout: format!("Autopilot {display} deleted.\n"),
        stderr: String::new(),
    })
}

pub(super) async fn run_autopilot_trigger(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let timeout = http_timeout(environment.raw("PATCHBAY_HTTP_TIMEOUT"))
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

#[derive(Debug, Deserialize, Serialize)]
struct AutopilotRunsEnvelope {
    runs: Vec<Value>,
    total: i64,
}

pub(super) async fn run_autopilot_runs(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    limit: i32,
    offset: i32,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, _) = resolve_autopilot_id(&client, &workspace_id, id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
    let mut query = form_urlencoded::Serializer::new(String::new());
    if limit > 0 {
        query.append_pair("limit", &limit.to_string());
    }
    if offset > 0 {
        query.append_pair("offset", &offset.to_string());
    }
    let query = query.finish();
    let path = if query.is_empty() {
        format!("/api/autopilots/{autopilot_id}/runs")
    } else {
        format!("/api/autopilots/{autopilot_id}/runs?{query}")
    };
    let response: AutopilotRunsEnvelope = client.get_json(&path).await.context("list runs")?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&response)?),
            OutputFormat::Table => format_autopilot_runs_table(&response.runs),
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
#[derive(Debug, Args)]
pub(super) struct AutopilotArgs {
    #[command(subcommand)]
    pub(super) command: AutopilotCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum AutopilotCommand {
    #[command(about = "List autopilots in the workspace")]
    List {
        #[arg(long, default_value = "", help = "Filter by status (active, paused)")]
        status: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
        #[arg(long, help = "Show full UUIDs in table output")]
        full_id: bool,
    },
    #[command(about = "Get autopilot details (includes triggers)")]
    Get {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
    #[command(about = "Create a new autopilot")]
    Create(AutopilotCreateArgs),
    #[command(about = "Update an autopilot")]
    Update(AutopilotUpdateArgs),
    #[command(about = "Delete an autopilot")]
    Delete {
        #[arg(value_name = "ID")]
        id: String,
    },
    #[command(about = "Manually trigger an autopilot to run once")]
    Trigger {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
    #[command(about = "List execution history for an autopilot")]
    Runs {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, default_value_t = 20, help = "Max number of runs to return")]
        limit: i32,
        #[arg(long, default_value_t = 0, help = "Pagination offset")]
        offset: i32,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Add a schedule or webhook trigger to an autopilot")]
    TriggerAdd(AutopilotTriggerAddArgs),
    #[command(about = "Update an existing trigger")]
    TriggerUpdate(AutopilotTriggerUpdateArgs),
    #[command(about = "Delete a trigger")]
    TriggerDelete {
        #[arg(value_name = "AUTOPILOT-ID")]
        autopilot_id: String,
        #[arg(value_name = "TRIGGER-ID")]
        trigger_id: String,
    },
    #[command(about = "Rotate the webhook URL of a webhook trigger")]
    TriggerRotateUrl(AutopilotTriggerRotateUrlArgs),
}

#[derive(Debug, Args)]
pub(super) struct AutopilotTriggerAddArgs {
    #[arg(value_name = "AUTOPILOT-ID")]
    pub(super) autopilot_id: String,
    #[arg(
        long,
        default_value = "schedule",
        help = "Trigger kind: schedule or webhook"
    )]
    pub(super) kind: String,
    #[arg(
        long,
        default_value = "",
        help = "Cron expression (required for --kind schedule)"
    )]
    pub(super) cron: String,
    #[arg(
        long,
        default_value = "",
        help = "IANA timezone (default UTC; schedule only)"
    )]
    pub(super) timezone: String,
    #[arg(long, default_value = "", help = "Optional human-readable label")]
    pub(super) label: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct AutopilotTriggerUpdateArgs {
    #[arg(value_name = "AUTOPILOT-ID")]
    pub(super) autopilot_id: String,
    #[arg(value_name = "TRIGGER-ID")]
    pub(super) trigger_id: String,
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    pub(super) enabled: Option<bool>,
    #[arg(long)]
    pub(super) cron: Option<String>,
    #[arg(long)]
    pub(super) timezone: Option<String>,
    #[arg(long)]
    pub(super) label: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct AutopilotTriggerRotateUrlArgs {
    #[arg(value_name = "AUTOPILOT-ID")]
    pub(super) autopilot_id: String,
    #[arg(value_name = "TRIGGER-ID")]
    pub(super) trigger_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
    #[arg(short = 'y', long, help = "Skip the interactive confirmation prompt")]
    pub(super) yes: bool,
}

#[derive(Debug, Args)]
pub(super) struct AutopilotCreateArgs {
    #[arg(long, help = "Autopilot title (required)")]
    pub(super) title: Option<String>,
    #[arg(
        long,
        default_value = "",
        help = "Autopilot description (used as task prompt)"
    )]
    pub(super) description: String,
    #[arg(long, help = "Assignee agent (name or ID) — required")]
    pub(super) agent: Option<String>,
    #[arg(long, help = "Execution mode: create_issue or run_only (required)")]
    pub(super) mode: Option<String>,
    #[arg(
        long,
        help = "Priority for created issues (none, low, medium, high, urgent)"
    )]
    pub(super) priority: Option<String>,
    #[arg(long, default_value = "", help = "Project ID (optional)")]
    pub(super) project: String,
    #[arg(
        long,
        default_value = "",
        help = "Template for issue titles (create_issue mode). Only {{date}} (UTC, YYYY-MM-DD) is interpolated; any other {{...}} token is rejected at create-time."
    )]
    pub(super) issue_title_template: String,
    #[arg(
        long,
        action = clap::ArgAction::Append,
        help = "Member subscriber to notify for issues this autopilot creates (name or user ID; repeatable)"
    )]
    pub(super) subscriber: Vec<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct AutopilotUpdateArgs {
    #[arg(value_name = "ID")]
    pub(super) id: String,
    #[arg(long)]
    pub(super) title: Option<String>,
    #[arg(long)]
    pub(super) description: Option<String>,
    #[arg(long, help = "New assignee agent (name or ID)")]
    pub(super) agent: Option<String>,
    #[arg(long, help = "New project ID (use empty string to clear)")]
    pub(super) project: Option<String>,
    #[arg(long)]
    pub(super) priority: Option<String>,
    #[arg(long, help = "New status (active, paused)")]
    pub(super) status: Option<String>,
    #[arg(long, help = "New execution mode (create_issue or run_only)")]
    pub(super) mode: Option<String>,
    #[arg(
        long,
        help = "New issue title template. Only {{date}} is interpolated."
    )]
    pub(super) issue_title_template: Option<String>,
    #[arg(long, action = clap::ArgAction::Append, help = "Replace subscribers with this member (repeatable)")]
    pub(super) subscriber: Vec<String>,
    #[arg(long, help = "Remove all autopilot subscribers")]
    pub(super) clear_subscribers: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}
