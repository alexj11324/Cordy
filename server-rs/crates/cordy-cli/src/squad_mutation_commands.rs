use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{
    encoded_path_segment, new_api_client, resolve_autopilot_agent, resolve_current_workspace_id,
    value_string, Cli, Environment, OutputFormat, RunOutput, SquadCreateArgs, SquadUpdateArgs,
};

pub(super) async fn run_squad_create(
    cli: &Cli,
    environment: &Environment,
    args: &SquadCreateArgs,
) -> Result<RunOutput> {
    let name = args.name.as_deref().unwrap_or_default().trim();
    if name.is_empty() {
        bail!("--name is required");
    }
    let leader = args.leader.as_deref().unwrap_or_default().trim();
    if leader.is_empty() {
        bail!("--leader is required (agent name or ID)");
    }
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let leader_id = resolve_autopilot_agent(&client, &workspace_id, leader)
        .await
        .context("resolve leader")?;
    let mut body = serde_json::Map::from_iter([
        ("name".into(), Value::String(name.into())),
        ("leader_id".into(), Value::String(leader_id)),
    ]);
    if !args.description.is_empty() {
        body.insert(
            "description".into(),
            Value::String(args.description.clone()),
        );
    }
    let squad: Value = client
        .post_json("/api/squads", &body)
        .await
        .context("create squad")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&squad)?),
            OutputFormat::Table => format!(
                "Squad created: {} ({})\n",
                value_string(&squad, "name"),
                value_string(&squad, "id")
            ),
        },
        stderr: String::new(),
    })
}

pub(super) async fn run_squad_update(
    cli: &Cli,
    environment: &Environment,
    args: &SquadUpdateArgs,
) -> Result<RunOutput> {
    let squad_id = args.squad_id.trim();
    if squad_id.is_empty() {
        bail!("squad ID must not be empty");
    }
    let mut body = serde_json::Map::new();
    if let Some(name) = &args.name {
        body.insert("name".into(), Value::String(name.clone()));
    }
    if let Some(description) = &args.description {
        body.insert("description".into(), Value::String(description.clone()));
    }
    if let Some(instructions) = &args.instructions {
        body.insert("instructions".into(), Value::String(instructions.clone()));
    }
    if let Some(avatar_url) = &args.avatar_url {
        body.insert("avatar_url".into(), Value::String(avatar_url.clone()));
    }
    if body.is_empty() && args.leader.is_none() {
        bail!(
            "no fields to update; use flags like --name, --description, --instructions, --leader"
        );
    }

    let client = new_api_client(cli, environment)?;
    if let Some(leader) = &args.leader {
        let leader = leader.trim();
        if leader.is_empty() {
            bail!("--leader must not be empty");
        }
        let workspace_id = resolve_current_workspace_id(cli, environment);
        let leader_id = resolve_autopilot_agent(&client, &workspace_id, leader)
            .await
            .context("resolve leader")?;
        body.insert("leader_id".into(), Value::String(leader_id));
    }
    if body.is_empty() {
        bail!(
            "no fields to update; use flags like --name, --description, --instructions, --leader"
        );
    }

    let squad: Value = client
        .put_json(
            &format!("/api/squads/{}", encoded_path_segment(squad_id)),
            &body,
        )
        .await
        .context("update squad")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&squad)?),
            OutputFormat::Table => format!(
                "Squad updated: {} ({})\n",
                value_string(&squad, "name"),
                value_string(&squad, "id")
            ),
        },
        stderr: String::new(),
    })
}

pub(super) async fn run_squad_delete(
    cli: &Cli,
    environment: &Environment,
    squad_id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let squad_id = squad_id.trim();
    if squad_id.is_empty() {
        bail!("squad ID must not be empty");
    }
    let client = new_api_client(cli, environment)?;
    client
        .delete(&format!("/api/squads/{}", encoded_path_segment(squad_id)))
        .await
        .context("delete squad")?;
    Ok(match output {
        OutputFormat::Json => RunOutput {
            stdout: format!(
                "{}\n",
                serde_json::to_string_pretty(&serde_json::json!({
                    "id": squad_id,
                    "deleted": true
                }))?
            ),
            stderr: String::new(),
        },
        OutputFormat::Table => RunOutput {
            stdout: String::new(),
            stderr: format!("Squad {squad_id} deleted.\n"),
        },
    })
}
