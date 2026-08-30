use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{
    encoded_path_segment, format_table, new_api_client, resolve_automation_agent,
    resolve_current_workspace_id, resolve_issue_ref, value_string, Cli, Environment, OutputFormat,
    RunOutput, TeamActivityArgs, TeamCreateArgs, TeamMemberAddArgs, TeamMemberRemoveArgs,
    TeamMemberSetRoleArgs, TeamUpdateArgs,
};

pub(super) async fn run_team_list(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let teams: Vec<Value> = client.get_json("/api/teams").await.context("list teams")?;
    if output == OutputFormat::Json {
        return Ok(RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(&teams)?),
            stderr: String::new(),
        });
    }
    if teams.is_empty() {
        return Ok(RunOutput {
            stdout: String::new(),
            stderr: "No teams found.\n".into(),
        });
    }
    Ok(RunOutput {
        stdout: format_team_list_table(&teams),
        stderr: String::new(),
    })
}

pub(super) async fn run_team_get(
    cli: &Cli,
    environment: &Environment,
    team_id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let team_id = team_id.trim();
    if team_id.is_empty() {
        bail!("team ID must not be empty");
    }
    let client = new_api_client(cli, environment)?;
    let team: Value = client
        .get_json(&format!("/api/teams/{}", encoded_path_segment(team_id)))
        .await
        .context("get team")?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&team)?),
            OutputFormat::Table => format_team_details_table(&team),
        },
        stderr: String::new(),
    })
}

pub(super) async fn run_team_create(
    cli: &Cli,
    environment: &Environment,
    args: &TeamCreateArgs,
) -> Result<RunOutput> {
    let name = args.name.as_deref().unwrap_or_default().trim();
    if name.is_empty() {
        bail!("--name is required");
    }
    let leader = args.leader.as_deref().unwrap_or_default().trim();
    if leader.is_empty() {
        bail!("--leader is required (name or ID)");
    }
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let leader_id = resolve_automation_agent(&client, &workspace_id, leader)
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
    let team: Value = client
        .post_json("/api/teams", &body)
        .await
        .context("create team")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&team)?),
            OutputFormat::Table => format!(
                "Team created: {} ({})\n",
                value_string(&team, "name"),
                value_string(&team, "id")
            ),
        },
        stderr: String::new(),
    })
}

pub(super) async fn run_team_update(
    cli: &Cli,
    environment: &Environment,
    args: &TeamUpdateArgs,
) -> Result<RunOutput> {
    let team_id = args.team_id.trim();
    if team_id.is_empty() {
        bail!("team ID must not be empty");
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
        let leader_id = resolve_automation_agent(&client, &workspace_id, leader)
            .await
            .context("resolve leader")?;
        body.insert("leader_id".into(), Value::String(leader_id));
    }
    if body.is_empty() {
        bail!(
            "no fields to update; use flags like --name, --description, --instructions, --leader"
        );
    }

    let team: Value = client
        .put_json(
            &format!("/api/teams/{}", encoded_path_segment(team_id)),
            &body,
        )
        .await
        .context("update team")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&team)?),
            OutputFormat::Table => format!(
                "Team updated: {} ({})\n",
                value_string(&team, "name"),
                value_string(&team, "id")
            ),
        },
        stderr: String::new(),
    })
}

pub(super) async fn run_team_delete(
    cli: &Cli,
    environment: &Environment,
    team_id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let team_id = team_id.trim();
    if team_id.is_empty() {
        bail!("team ID must not be empty");
    }
    let client = new_api_client(cli, environment)?;
    client
        .delete(&format!("/api/teams/{}", encoded_path_segment(team_id)))
        .await
        .context("delete team")?;
    Ok(match output {
        OutputFormat::Json => RunOutput {
            stdout: format!(
                "{}\n",
                serde_json::to_string_pretty(&serde_json::json!({
                    "id": team_id,
                    "deleted": true
                }))?
            ),
            stderr: String::new(),
        },
        OutputFormat::Table => RunOutput {
            stdout: String::new(),
            stderr: format!("Team {team_id} deleted.\n"),
        },
    })
}

pub(super) async fn run_team_member_list(
    cli: &Cli,
    environment: &Environment,
    team_id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let team_id = team_id.trim();
    if team_id.is_empty() {
        bail!("team ID must not be empty");
    }
    let client = new_api_client(cli, environment)?;
    let members: Vec<Value> = client
        .get_json(&format!(
            "/api/teams/{}/members",
            encoded_path_segment(team_id)
        ))
        .await
        .context("list team members")?;
    render_team_member_output(&members, output)
}

pub(super) async fn run_team_member_add(
    cli: &Cli,
    environment: &Environment,
    args: &TeamMemberAddArgs,
) -> Result<RunOutput> {
    let team_id = args.team_id.trim();
    if team_id.is_empty() {
        bail!("team ID must not be empty");
    }
    let member_id = args.member_id.as_deref().unwrap_or_default().trim();
    if member_id.is_empty() {
        bail!("--member-id is required");
    }
    if !matches!(args.member_type.as_str(), "agent" | "member") {
        bail!("--type must be 'agent' or 'member'");
    }
    let client = new_api_client(cli, environment)?;
    let result: Value = client
        .post_json(
            &format!("/api/teams/{}/members", encoded_path_segment(team_id)),
            &serde_json::json!({
                "member_type": args.member_type.as_str(),
                "member_id": member_id,
                "role": args.role.as_str(),
            }),
        )
        .await
        .context("add team member")?;
    Ok(match args.output {
        OutputFormat::Json => RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(&result)?),
            stderr: String::new(),
        },
        OutputFormat::Table => RunOutput {
            stdout: String::new(),
            stderr: format!("Member {member_id} added to team.\n"),
        },
    })
}

pub(super) async fn run_team_member_set_role(
    cli: &Cli,
    environment: &Environment,
    args: &TeamMemberSetRoleArgs,
) -> Result<RunOutput> {
    let team_id = args.team_id.trim();
    if team_id.is_empty() {
        bail!("team ID must not be empty");
    }
    let member_id = args.member_id.as_deref().unwrap_or_default().trim();
    if member_id.is_empty() {
        bail!("--member-id is required");
    }
    if !matches!(args.member_type.as_str(), "agent" | "member") {
        bail!("--member-type must be 'agent' or 'member'");
    }
    let role = args.role.as_deref().unwrap_or_default().trim();
    if role.is_empty() {
        bail!("--role is required");
    }
    let client = new_api_client(cli, environment)?;
    let result: Value = client
        .patch_json(
            &format!("/api/teams/{}/members/role", encoded_path_segment(team_id)),
            &serde_json::json!({
                "member_type": args.member_type.as_str(),
                "member_id": member_id,
                "role": role,
            }),
        )
        .await
        .context("set member role")?;
    Ok(match args.output {
        OutputFormat::Json => RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(&result)?),
            stderr: String::new(),
        },
        OutputFormat::Table => RunOutput {
            stdout: String::new(),
            stderr: format!("Member {member_id} role updated to {role}.\n"),
        },
    })
}

pub(super) async fn run_team_member_remove(
    cli: &Cli,
    environment: &Environment,
    args: &TeamMemberRemoveArgs,
) -> Result<RunOutput> {
    let team_id = args.team_id.trim();
    if team_id.is_empty() {
        bail!("team ID must not be empty");
    }
    let member_id = args.member_id.as_deref().unwrap_or_default().trim();
    if member_id.is_empty() {
        bail!("--member-id is required");
    }
    if !matches!(args.member_type.as_str(), "agent" | "member") {
        bail!("--type must be 'agent' or 'member'");
    }
    let client = new_api_client(cli, environment)?;
    client
        .delete_json_with_body(
            &format!("/api/teams/{}/members", encoded_path_segment(team_id)),
            &serde_json::json!({
                "member_type": args.member_type.as_str(),
                "member_id": member_id,
            }),
        )
        .await
        .context("remove team member")?;
    let result = serde_json::json!({
        "team_id": team_id,
        "member_id": member_id,
        "removed": true,
    });
    Ok(match args.output {
        OutputFormat::Json => RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(&result)?),
            stderr: String::new(),
        },
        OutputFormat::Table => RunOutput {
            stdout: String::new(),
            stderr: format!("Member {member_id} removed from team.\n"),
        },
    })
}

pub(super) async fn run_team_activity(
    cli: &Cli,
    environment: &Environment,
    args: &TeamActivityArgs,
) -> Result<RunOutput> {
    let outcome = args.outcome.as_str();
    if !matches!(outcome, "action" | "no_action" | "failed") {
        bail!("invalid outcome {outcome:?}; valid values: action, no_action, failed");
    }
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let result: Value = client
        .post_json(
            &format!("/api/issues/{issue_id}/team-evaluated"),
            &serde_json::json!({
                "outcome": outcome,
                "reason": args.reason.as_str(),
            }),
        )
        .await
        .context("record evaluation")?;
    let issue_display = args.issue_id.trim();
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
            OutputFormat::Table => String::new(),
        },
        stderr: format!("Team evaluation recorded: {outcome} (issue {issue_display})\n"),
    })
}

pub(super) fn render_team_member_output(
    members: &[Value],
    output: OutputFormat,
) -> Result<RunOutput> {
    if output == OutputFormat::Json {
        return Ok(RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(members)?),
            stderr: String::new(),
        });
    }
    if members.is_empty() {
        return Ok(RunOutput {
            stdout: String::new(),
            stderr: "No members found.\n".into(),
        });
    }
    Ok(RunOutput {
        stdout: format_team_member_table(members),
        stderr: String::new(),
    })
}

pub(super) fn format_team_member_table(members: &[Value]) -> String {
    let mut rows = vec![vec!["MEMBER ID".into(), "TYPE".into(), "ROLE".into()]];
    rows.extend(members.iter().map(|member| {
        vec![
            value_string(member, "member_id"),
            value_string(member, "member_type"),
            value_string(member, "role"),
        ]
    }));
    format_table(&rows)
}

pub(super) fn format_team_list_table(teams: &[Value]) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "NAME".into(),
        "LEADER ID".into(),
        "MEMBERS".into(),
    ]];
    rows.extend(teams.iter().map(|team| {
        vec![
            value_string(team, "id"),
            value_string(team, "name"),
            value_string(team, "leader_id"),
            team_member_count_display(team),
        ]
    }));
    format_table(&rows)
}

pub(super) fn team_member_count_display(team: &Value) -> String {
    let Some(count) = team.get("member_count") else {
        return "-".into();
    };
    if let Some(count) = count.as_u64().filter(|count| *count > 0) {
        return count.to_string();
    }
    if let Some(count) = count.as_i64().filter(|count| *count > 0) {
        return count.to_string();
    }
    "-".into()
}

pub(super) fn format_team_details_table(team: &Value) -> String {
    let mut output = format!(
        "ID:           {}\nName:         {}\nDescription:  {}\nLeader ID:    {}\nCreated:      {}\n",
        value_string(team, "id"),
        value_string(team, "name"),
        value_string(team, "description"),
        value_string(team, "leader_id"),
        value_string(team, "created_at"),
    );
    let instructions = value_string(team, "instructions");
    if !instructions.is_empty() {
        output.push_str(&format!("Instructions: {}\n", instructions));
    }
    output
}
