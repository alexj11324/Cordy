use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

use super::{
    format_table, new_api_client, required_workspace_id, value_string, ApiClient, Cli, Environment,
    OutputFormat, RunOutput,
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
pub(super) struct RepoMutationResult {
    pub(super) workspace_id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) added: Vec<WorkspaceRepo>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) updated: Vec<WorkspaceRepo>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) removed: Vec<WorkspaceRepo>,
    pub(super) repos: Vec<WorkspaceRepo>,
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

pub(super) async fn patch_workspace_repos(
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

pub(super) fn format_repo_list(repos: &[WorkspaceRepo]) -> String {
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

pub(super) fn repo_checkout_retry_delay(
    value: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> std::time::Duration {
    const DEFAULT_DELAY: std::time::Duration = std::time::Duration::from_secs(1);
    const MAX_DELAY: std::time::Duration = std::time::Duration::from_secs(30);
    let value = value.trim();
    if let Ok(seconds) = value.parse::<i64>() {
        if seconds >= 0 {
            return std::time::Duration::from_secs(seconds as u64).min(MAX_DELAY);
        }
    }
    if let Ok(retry_at) = chrono::DateTime::parse_from_rfc2822(value) {
        let delay = retry_at.with_timezone(&chrono::Utc) - now;
        return delay.to_std().unwrap_or_default().min(MAX_DELAY);
    }
    DEFAULT_DELAY
}

pub(super) async fn run_repo_checkout(
    environment: &Environment,
    repo_url: &str,
    checkout_ref: Option<&str>,
) -> Result<RunOutput> {
    let daemon_port = environment.raw("CORDY_DAEMON_PORT").unwrap_or_default();
    if daemon_port.is_empty() {
        bail!(
            "CORDY_DAEMON_PORT not set (this command is intended to be run by an agent inside a daemon task)"
        );
    }
    let token = environment.raw("CORDY_TOKEN").unwrap_or_default();
    if token.is_empty() {
        bail!("CORDY_TOKEN not set (repo checkout requires the active task credential)");
    }
    let body = serde_json::json!({
        "url":repo_url,
        "workspace_id":environment.raw("CORDY_WORKSPACE_ID").unwrap_or_default(),
        "workdir":environment.current_dir(),
        "ref":checkout_ref.unwrap_or_default(),
        "agent_name":environment.raw("CORDY_AGENT_NAME").unwrap_or_default(),
        "task_id":environment.raw("CORDY_TASK_ID").unwrap_or_default(),
        "checkout_mode":environment.raw("CORDY_REPO_CHECKOUT_MODE").unwrap_or_default().trim(),
        "retry_busy":true
    });
    let checkout_url = format!("http://127.0.0.1:{daemon_port}/repo/checkout");
    let client = reqwest::Client::new();
    let checkout = async {
        loop {
            let response = client
                .post(&checkout_url)
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .context("connect to daemon")?;
            let status = response.status();
            let retryable = response
                .headers()
                .get("X-Cordy-Retryable")
                .and_then(|value| value.to_str().ok())
                == Some("repo-busy");
            let retry_after = response
                .headers()
                .get("Retry-After")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let response_body = response
                .text()
                .await
                .context("read daemon checkout response")?;
            if status == reqwest::StatusCode::SERVICE_UNAVAILABLE && retryable {
                tokio::time::sleep(repo_checkout_retry_delay(&retry_after, chrono::Utc::now()))
                    .await;
                continue;
            }
            if status != reqwest::StatusCode::OK {
                bail!("checkout failed: {response_body}");
            }
            let result: Value = serde_json::from_str(&response_body).context("parse response")?;
            let path = value_string(&result, "path");
            let branch = value_string(&result, "branch_name");
            return Ok(RunOutput {
                stdout: format!("{path}\n"),
                stderr: format!("Checked out {repo_url} → {path} (branch: {branch})\n"),
            });
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(5 * 60), checkout)
        .await
        .map_err(|_| anyhow::anyhow!("connect to daemon: deadline exceeded"))?
}
#[derive(Debug, Args)]
struct RepoArgs {
    #[command(subcommand)]
    command: RepoCommand,
}

#[derive(Debug, Subcommand)]
enum RepoCommand {
    #[command(about = "List workspace repositories")]
    List {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Add repositories to the workspace registry")]
    Add(RepoMutationArgs),
    #[command(
        alias = "rm",
        about = "Remove repositories from the workspace registry"
    )]
    Remove(RepoRemoveArgs),
    #[command(about = "Check out a repository into the working directory")]
    Checkout {
        #[arg(value_name = "URL")]
        url: String,
        #[arg(
            long = "ref",
            help = "branch, tag, or commit to check out instead of the remote default branch"
        )]
        checkout_ref: Option<String>,
    },
}

#[derive(Debug, Args)]
struct RepoMutationArgs {
    #[arg(value_name = "URL")]
    urls: Vec<String>,
    #[arg(long = "url", action = clap::ArgAction::Append, help = "Repository URL (may be repeated)")]
    flag_urls: Vec<String>,
    #[arg(long, help = "Optional description; only valid when adding one URL")]
    description: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct RepoRemoveArgs {
    #[arg(value_name = "URL")]
    urls: Vec<String>,
    #[arg(long = "url", action = clap::ArgAction::Append, help = "Repository URL to remove (may be repeated)")]
    flag_urls: Vec<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}
