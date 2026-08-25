use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use super::{
    new_api_client, output_runtime_profiles, required_workspace_id, Cli, Environment, OutputFormat,
    RunOutput,
};

#[derive(Debug, Deserialize)]
pub(super) struct RuntimeProfileListResponse {
    #[serde(default)]
    pub(super) runtime_profiles: Vec<Value>,
}

pub(super) fn runtime_profiles_path(workspace_id: &str) -> String {
    format!("/api/workspaces/{workspace_id}/runtime-profiles")
}

pub(super) async fn run_runtime_profile_list(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = required_workspace_id(cli, environment)?;
    let response: RuntimeProfileListResponse = client
        .get_json(&runtime_profiles_path(&workspace_id))
        .await
        .context("list runtime profiles")?;
    output_runtime_profiles(&response.runtime_profiles, output, false)
}
