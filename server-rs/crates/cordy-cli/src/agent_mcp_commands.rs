use anyhow::{Context, Result};
use url::Url;

use super::{
    format_workspace_mcp_servers, new_api_client, AgentMcpListArgs, AgentMcpMutationArgs, Cli,
    Environment, RunOutput, WorkspaceMcpServer,
};

#[derive(Clone, Copy)]
pub(super) enum AgentMcpAction {
    Add,
    Enable,
    Disable,
    Remove,
}

pub(super) fn agent_mcp_path(agent_id: &str, suffix: &[&str]) -> String {
    let mut url = Url::parse("http://localhost").expect("constant URL");
    {
        let mut segments = url.path_segments_mut().expect("hierarchical URL");
        segments.clear();
        segments.extend(["api", "agents", agent_id.trim(), "mcp-servers"]);
        segments.extend(suffix.iter().copied());
    }
    url.path().into()
}

pub(super) async fn run_agent_mcp_list(
    cli: &Cli,
    environment: &Environment,
    args: &AgentMcpListArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let servers: Vec<WorkspaceMcpServer> = client
        .get_json(&agent_mcp_path(&args.agent_id, &[]))
        .await
        .context("list agent mcp servers")?;
    Ok(RunOutput {
        stdout: format_workspace_mcp_servers(&servers, args.output)?,
        stderr: String::new(),
    })
}

pub(super) async fn run_agent_mcp_mutation(
    cli: &Cli,
    environment: &Environment,
    args: &AgentMcpMutationArgs,
    action: AgentMcpAction,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let agent_id = args.agent_id.trim();
    let server_id = args.server_id.trim();
    let servers: Vec<WorkspaceMcpServer> = match action {
        AgentMcpAction::Add => client
            .post_json(
                &agent_mcp_path(agent_id, &[]),
                &serde_json::json!({"server_id":server_id}),
            )
            .await
            .context("add agent mcp server")?,
        AgentMcpAction::Enable | AgentMcpAction::Disable => client
            .put_json(
                &agent_mcp_path(agent_id, &[server_id, "enabled"]),
                &serde_json::json!({"enabled":matches!(action, AgentMcpAction::Enable)}),
            )
            .await
            .context("update agent mcp server")?,
        AgentMcpAction::Remove => client
            .delete_json(&agent_mcp_path(agent_id, &[server_id]))
            .await
            .context("remove agent mcp server")?,
    };
    Ok(RunOutput {
        stdout: format_workspace_mcp_servers(&servers, args.output)?,
        stderr: String::new(),
    })
}
