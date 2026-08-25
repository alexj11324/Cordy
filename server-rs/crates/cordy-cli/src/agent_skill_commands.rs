use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{
    format_table, new_api_client, value_string, AgentSkillsMutationArgs, Cli, Environment,
    OutputFormat, RunOutput,
};

pub(super) async fn run_agent_skills_list(
    cli: &Cli,
    environment: &Environment,
    agent_id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let skills: Vec<Value> = client
        .get_json(&format!("/api/agents/{agent_id}/skills"))
        .await
        .context("list agent skills")?;
    Ok(RunOutput {
        stdout: format_agent_skills(&skills, output, None)?,
        stderr: String::new(),
    })
}

pub(super) async fn run_agent_skills_mutation(
    cli: &Cli,
    environment: &Environment,
    args: &AgentSkillsMutationArgs,
    additive: bool,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let supplied = args.skill_ids.as_ref().with_context(|| {
        if additive {
            "--skill-ids is required (comma-separated skill IDs)"
        } else {
            "--skill-ids is required (comma-separated skill IDs; use --skill-ids '' to clear all)"
        }
    })?;
    let skill_ids = supplied
        .iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();
    if additive && skill_ids.is_empty() {
        bail!("--skill-ids must include at least one skill ID");
    }
    let path = if additive {
        format!("/api/agents/{}/skills/add", args.agent_id)
    } else {
        format!("/api/agents/{}/skills", args.agent_id)
    };
    let body = serde_json::json!({"skill_ids":skill_ids});
    let skills: Vec<Value> = if additive {
        client
            .post_json(&path, &body)
            .await
            .context("add agent skills")?
    } else {
        client
            .put_json(&path, &body)
            .await
            .context("set agent skills")?
    };
    Ok(RunOutput {
        stdout: format_agent_skills(&skills, args.output, Some(&args.agent_id))?,
        stderr: String::new(),
    })
}

fn format_agent_skills(
    skills: &[Value],
    output: OutputFormat,
    empty_agent_id: Option<&str>,
) -> Result<String> {
    if output == OutputFormat::Json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(skills)?));
    }
    if skills.is_empty() {
        if let Some(agent_id) = empty_agent_id {
            return Ok(format!("No skills assigned to agent {agent_id}\n"));
        }
    }
    let mut rows = vec![vec!["ID".into(), "NAME".into(), "DESCRIPTION".into()]];
    rows.extend(skills.iter().map(|skill| {
        vec![
            value_string(skill, "id"),
            value_string(skill, "name"),
            value_string(skill, "description"),
        ]
    }));
    Ok(format_table(&rows))
}
