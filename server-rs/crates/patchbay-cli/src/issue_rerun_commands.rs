use anyhow::{Context, Result};
use serde_json::Value;

use super::{
    load_issue_actor_names, new_api_client, resolve_current_workspace_id, resolve_issue_ref,
    value_string, Cli, Environment, IssueActorNames, IssueRerunArgs, OutputFormat, RunOutput,
};

pub(super) async fn run_issue_rerun(
    cli: &Cli,
    environment: &Environment,
    args: &IssueRerunArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let task: Value = client
        .post_json(
            &format!("/api/issues/{issue_id}/rerun"),
            &serde_json::Map::<String, Value>::new(),
        )
        .await
        .context("rerun issue")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&task)?),
        OutputFormat::Table => {
            let agent_id = value_string(&task, "agent_id");
            let synthetic = [serde_json::json!({
                "assignee_type":"agent","assignee_id":agent_id.clone()
            })];
            let workspace_id = resolve_current_workspace_id(cli, environment);
            let actors: IssueActorNames =
                load_issue_actor_names(&client, &workspace_id, &synthetic).await;
            let agent = actors
                .0
                .get(&format!("agent:{agent_id}"))
                .cloned()
                .unwrap_or(agent_id);
            format!(
                "Re-enqueued task {} on agent {agent}\n",
                value_string(&task, "id")
            )
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}
