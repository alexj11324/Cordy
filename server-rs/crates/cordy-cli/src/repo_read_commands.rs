use anyhow::Result;

use super::repo_mutation_commands::{WorkspaceRepo, fetch_repo_workspace};
use super::{
    Cli, Environment, OutputFormat, RunOutput, format_table, new_api_client, required_workspace_id,
};

fn format_repo_list(repos: &[WorkspaceRepo]) -> String {
    let mut rows = vec![vec!["URL".into(), "DESCRIPTION".into()]];
    rows.extend(
        repos
            .iter()
            .map(|repo| vec![repo.url.clone(), repo.description.clone()]),
    );
    format_table(&rows)
}

pub(super) async fn run_repo_list(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
) -> Result<RunOutput> {
    let workspace_id = required_workspace_id(cli, environment)?;
    let client = new_api_client(cli, environment)?;
    let workspace = fetch_repo_workspace(&client, &workspace_id).await?;
    Ok(match output {
        OutputFormat::Json => RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(&workspace.repos)?),
            stderr: String::new(),
        },
        OutputFormat::Table if workspace.repos.is_empty() => RunOutput {
            stdout: String::new(),
            stderr: "No repositories found.\n".into(),
        },
        OutputFormat::Table => RunOutput {
            stdout: format_repo_list(&workspace.repos),
            stderr: String::new(),
        },
    })
}
