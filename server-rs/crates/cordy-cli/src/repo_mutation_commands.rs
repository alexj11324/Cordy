use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use super::{
    format_table, new_api_client, required_workspace_id, ApiClient, Cli, Environment, OutputFormat,
    RepoMutationArgs, RepoRemoveArgs, RunOutput,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct WorkspaceRepo {
    pub(super) url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(super) description: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct RepoWorkspace {
    pub(super) id: String,
    #[serde(default)]
    pub(super) repos: Vec<WorkspaceRepo>,
}

#[derive(Debug, Serialize)]
struct RepoMutationResult {
    workspace_id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    added: Vec<WorkspaceRepo>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    updated: Vec<WorkspaceRepo>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    removed: Vec<WorkspaceRepo>,
    repos: Vec<WorkspaceRepo>,
}

pub(super) fn repo_urls(flag_urls: &[String], positional: &[String]) -> Result<Vec<String>> {
    let mut raw = Vec::with_capacity(flag_urls.len() + positional.len());
    raw.extend(flag_urls.iter());
    raw.extend(positional.iter());
    if raw.is_empty() {
        bail!("at least one repository URL is required");
    }
    let mut seen = HashSet::new();
    let mut urls = Vec::new();
    for url in raw {
        let url = url.trim();
        if url.is_empty() {
            bail!("repository URL cannot be empty");
        }
        if seen.insert(url.to_string()) {
            urls.push(url.to_string());
        }
    }
    Ok(urls)
}

pub(super) async fn fetch_repo_workspace(
    client: &ApiClient,
    workspace_id: &str,
) -> Result<RepoWorkspace> {
    client
        .get_json(&format!("/api/workspaces/{workspace_id}"))
        .await
        .context("get workspace")
}

async fn patch_workspace_repos(
    client: &ApiClient,
    workspace_id: &str,
    repos: &[WorkspaceRepo],
) -> Result<RepoWorkspace> {
    client
        .patch_json(
            &format!("/api/workspaces/{workspace_id}"),
            &serde_json::json!({"repos":repos}),
        )
        .await
        .context("update workspace repos")
}

pub(super) async fn run_repo_add(
    cli: &Cli,
    environment: &Environment,
    args: &RepoMutationArgs,
) -> Result<RunOutput> {
    let urls = repo_urls(&args.flag_urls, &args.urls)?;
    if args.description.is_some() && urls.len() > 1 {
        bail!("--description can only be used when adding one repository URL");
    }
    let workspace_id = required_workspace_id(cli, environment)?;
    let client = new_api_client(cli, environment)?;
    let mut workspace = fetch_repo_workspace(&client, &workspace_id).await?;
    let mut index_by_url = workspace
        .repos
        .iter()
        .enumerate()
        .map(|(index, repo)| (repo.url.clone(), index))
        .collect::<HashMap<_, _>>();
    let mut added = Vec::new();
    let mut updated = Vec::new();
    for url in urls {
        if let Some(index) = index_by_url.get(&url).copied() {
            if let Some(description) = &args.description {
                if workspace.repos[index].description != *description {
                    workspace.repos[index].description = description.clone();
                    updated.push(workspace.repos[index].clone());
                }
            }
            continue;
        }
        let repo = WorkspaceRepo {
            url: url.clone(),
            description: args.description.clone().unwrap_or_default(),
        };
        index_by_url.insert(url, workspace.repos.len());
        workspace.repos.push(repo.clone());
        added.push(repo);
    }
    if !added.is_empty() || !updated.is_empty() {
        workspace = patch_workspace_repos(&client, &workspace_id, &workspace.repos).await?;
    }
    let result = RepoMutationResult {
        workspace_id: workspace.id,
        added,
        updated,
        removed: Vec::new(),
        repos: workspace.repos,
    };
    let stdout =
        match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
            OutputFormat::Table if result.added.is_empty() && result.updated.is_empty() => {
                "No repository changes.\n".into()
            }
            OutputFormat::Table => {
                let mut rows = vec![vec!["ACTION".into(), "URL".into(), "DESCRIPTION".into()]];
                rows.extend(
                    result.added.iter().map(|repo| {
                        vec!["added".into(), repo.url.clone(), repo.description.clone()]
                    }),
                );
                rows.extend(result.updated.iter().map(|repo| {
                    vec!["updated".into(), repo.url.clone(), repo.description.clone()]
                }));
                format_table(&rows)
            }
        };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) async fn run_repo_remove(
    cli: &Cli,
    environment: &Environment,
    args: &RepoRemoveArgs,
) -> Result<RunOutput> {
    let urls = repo_urls(&args.flag_urls, &args.urls)?;
    let workspace_id = required_workspace_id(cli, environment)?;
    let client = new_api_client(cli, environment)?;
    let workspace = fetch_repo_workspace(&client, &workspace_id).await?;
    let remove_set = urls.iter().cloned().collect::<HashSet<_>>();
    let (removed, repos): (Vec<_>, Vec<_>) = workspace
        .repos
        .into_iter()
        .partition(|repo| remove_set.contains(&repo.url));
    let removed_set = removed
        .iter()
        .map(|repo| repo.url.as_str())
        .collect::<HashSet<_>>();
    let missing = urls
        .iter()
        .filter(|url| !removed_set.contains(url.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        bail!(
            "repository not found in workspace registry: {}",
            missing.join(", ")
        );
    }
    let workspace = patch_workspace_repos(&client, &workspace_id, &repos).await?;
    let result = RepoMutationResult {
        workspace_id: workspace.id,
        added: Vec::new(),
        updated: Vec::new(),
        removed,
        repos: workspace.repos,
    };
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
            OutputFormat::Table => {
                let mut rows = vec![vec!["REMOVED URL".into(), "DESCRIPTION".into()]];
                rows.extend(
                    result
                        .removed
                        .iter()
                        .map(|repo| vec![repo.url.clone(), repo.description.clone()]),
                );
                format_table(&rows)
            }
        },
        stderr: String::new(),
    })
}
