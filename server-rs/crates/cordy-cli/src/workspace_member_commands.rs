use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::workspace_commands::resolve_workspace_arg;
use super::{
    format_table, new_api_client, value_string, Cli, Environment, OutputFormat, RunOutput,
    WorkspaceMemberInviteArgs,
};

pub(super) fn format_workspace_members(members: &[Value]) -> String {
    let mut rows = vec![vec![
        "USER ID".into(),
        "NAME".into(),
        "EMAIL".into(),
        "ROLE".into(),
    ]];
    rows.extend(members.iter().map(|member| {
        vec![
            value_string(member, "user_id"),
            value_string(member, "name"),
            value_string(member, "email"),
            value_string(member, "role"),
        ]
    }));
    format_table(&rows)
}

pub(super) async fn run_workspace_member_list(
    cli: &Cli,
    environment: &Environment,
    workspace: Option<&str>,
    output: OutputFormat,
) -> Result<RunOutput> {
    let workspace_id = resolve_workspace_arg(cli, environment, workspace).await?;
    if workspace_id.is_empty() {
        bail!(
            "workspace ID is required: pass an id/slug/prefix as argument or set CORDY_WORKSPACE_ID"
        );
    }
    let client = new_api_client(cli, environment)?;
    let members: Vec<Value> = client
        .get_json(&format!("/api/workspaces/{workspace_id}/members"))
        .await
        .context("list members")?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&members)?),
            OutputFormat::Table => format_workspace_members(&members),
        },
        stderr: String::new(),
    })
}

pub(super) fn normalize_workspace_invite_role(role: &str) -> Result<String> {
    let role = match role.trim().to_ascii_lowercase() {
        role if role.is_empty() => "member".into(),
        role => role,
    };
    match role.as_str() {
        "member" | "admin" => Ok(role),
        "owner" => bail!("cannot invite as owner; use --role member or --role admin"),
        _ => bail!("invalid --role {role:?}; expected member or admin"),
    }
}

pub(super) async fn run_workspace_member_invite(
    cli: &Cli,
    environment: &Environment,
    args: &WorkspaceMemberInviteArgs,
) -> Result<RunOutput> {
    let email = args.email.trim().to_ascii_lowercase();
    if email.is_empty() {
        bail!("email is required");
    }
    let role = normalize_workspace_invite_role(&args.role)?;
    let workspace_id = resolve_workspace_arg(cli, environment, args.workspace.as_deref()).await?;
    if workspace_id.is_empty() {
        bail!(
            "workspace ID is required: pass an id/slug/prefix as argument or set CORDY_WORKSPACE_ID"
        );
    }
    let client = new_api_client(cli, environment)?;
    let invitation: Value = client
        .post_json(
            &format!("/api/workspaces/{workspace_id}/members"),
            &serde_json::json!({"email":email,"role":role}),
        )
        .await
        .context("invite member")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&invitation)?),
            OutputFormat::Table => format!(
                "Invitation sent to {} (role: {}, status: {})\n",
                value_string(&invitation, "invitee_email"),
                value_string(&invitation, "role"),
                value_string(&invitation, "status")
            ),
        },
        stderr: String::new(),
    })
}
