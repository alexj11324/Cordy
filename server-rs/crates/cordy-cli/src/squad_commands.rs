use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{
    encoded_path_segment, format_table, new_api_client, resolve_issue_ref, value_string, Cli,
    Environment, OutputFormat, RunOutput, SquadActivityArgs, SquadMemberAddArgs,
    SquadMemberRemoveArgs, SquadMemberSetRoleArgs,
};

pub(super) async fn run_squad_member_list(
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
    let members: Vec<Value> = client
        .get_json(&format!(
            "/api/squads/{}/members",
            encoded_path_segment(squad_id)
        ))
        .await
        .context("list squad members")?;
    render_squad_member_output(&members, output)
}

pub(super) async fn run_squad_member_add(
    cli: &Cli,
    environment: &Environment,
    args: &SquadMemberAddArgs,
) -> Result<RunOutput> {
    let squad_id = args.squad_id.trim();
    if squad_id.is_empty() {
        bail!("squad ID must not be empty");
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
            &format!("/api/squads/{}/members", encoded_path_segment(squad_id)),
            &serde_json::json!({
                "member_type": args.member_type.as_str(),
                "member_id": member_id,
                "role": args.role.as_str(),
            }),
        )
        .await
        .context("add squad member")?;
    Ok(match args.output {
        OutputFormat::Json => RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(&result)?),
            stderr: String::new(),
        },
        OutputFormat::Table => RunOutput {
            stdout: String::new(),
            stderr: format!("Member {member_id} added to squad.\n"),
        },
    })
}

pub(super) async fn run_squad_member_set_role(
    cli: &Cli,
    environment: &Environment,
    args: &SquadMemberSetRoleArgs,
) -> Result<RunOutput> {
    let squad_id = args.squad_id.trim();
    if squad_id.is_empty() {
        bail!("squad ID must not be empty");
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
            &format!(
                "/api/squads/{}/members/role",
                encoded_path_segment(squad_id)
            ),
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

pub(super) async fn run_squad_member_remove(
    cli: &Cli,
    environment: &Environment,
    args: &SquadMemberRemoveArgs,
) -> Result<RunOutput> {
    let squad_id = args.squad_id.trim();
    if squad_id.is_empty() {
        bail!("squad ID must not be empty");
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
            &format!("/api/squads/{}/members", encoded_path_segment(squad_id)),
            &serde_json::json!({
                "member_type": args.member_type.as_str(),
                "member_id": member_id,
            }),
        )
        .await
        .context("remove squad member")?;
    let result = serde_json::json!({
        "squad_id": squad_id,
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
            stderr: format!("Member {member_id} removed from squad.\n"),
        },
    })
}

pub(super) async fn run_squad_activity(
    cli: &Cli,
    environment: &Environment,
    args: &SquadActivityArgs,
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
            &format!("/api/issues/{issue_id}/squad-evaluated"),
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
        stderr: format!("Squad evaluation recorded: {outcome} (issue {issue_display})\n"),
    })
}

pub(super) fn render_squad_member_output(
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
        stdout: format_squad_member_table(members),
        stderr: String::new(),
    })
}

pub(super) fn format_squad_member_table(members: &[Value]) -> String {
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
