//! Safe local GitHub CLI capability.
//!
//! The daemon runs gh in the active task checkout, never asks for a token,
//! and only returns sanitized status/PR metadata. Tests pass an explicit
//! fixture executable; no ambient user installation is discovered by tests.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum GhError {
    #[error("GitHub CLI was not found")]
    NotFound,
    #[error("GitHub CLI exited with status {status}: {message}")]
    CommandFailed { status: i32, message: String },
    #[error("GitHub CLI output was invalid: {0}")]
    InvalidOutput(String),
    #[error("GitHub CLI could not run: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GhStatus {
    pub path: String,
    pub hosts: serde_json::Value,
    pub login: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GhPullRequest {
    pub url: String,
    pub number: Option<u64>,
    pub repository: Option<String>,
    pub head: Option<String>,
    pub base: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GhCapability {
    pub status: GhStatus,
    pub task_id: String,
    pub workdir: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhPrCreateRequest {
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub head: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhPrViewRequest {
    pub number: String,
}

pub fn discover_gh_path(explicit: Option<&Path>) -> Result<PathBuf, GhError> {
    if let Some(path) = explicit {
        return path.is_file().then(|| path.to_path_buf()).ok_or(GhError::NotFound);
    }
    if let Some(path) = std::env::var_os("PATCHBAY_GH_PATH").map(PathBuf::from) {
        return path.is_file().then_some(path).ok_or(GhError::NotFound);
    }
    let names = if cfg!(windows) {
        vec![r"C:\Program Files\GitHub CLI\gh.exe", r"C:\Program Files\gh\bin\gh.exe"]
    } else {
        vec![
            "/usr/bin/gh",
            "/usr/local/bin/gh",
            "/opt/homebrew/bin/gh",
            "/home/linuxbrew/.linuxbrew/bin/gh",
        ]
    };
    names
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .ok_or(GhError::NotFound)
}

async fn run_gh(path: &Path, workdir: &Path, args: &[&str]) -> Result<String, GhError> {
    let output = Command::new(path)
        .args(args)
        .current_dir(workdir)
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;
    if !output.status.success() {
        return Err(GhError::CommandFailed {
            status: output.status.code().unwrap_or(-1),
            message: sanitize_output(&String::from_utf8_lossy(&output.stderr)),
        });
    }
    Ok(sanitize_output(&String::from_utf8_lossy(&output.stdout)))
}

fn sanitize_output(output: &str) -> String {
    output
        .lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            !lower.contains("token=")
                && !lower.contains("oauth_token")
                && !lower.contains("password=")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub async fn auth_status(path: &Path, workdir: &Path) -> Result<GhStatus, GhError> {
    let hosts_raw = run_gh(path, workdir, &["auth", "status", "--json", "hosts"]).await?;
    let hosts: serde_json::Value = serde_json::from_str(&hosts_raw)
        .map_err(|error| GhError::InvalidOutput(error.to_string()))?;
    let login = run_gh(path, workdir, &["api", "user", "--jq", ".login"]).await?;
    let login = login.trim().to_string();
    if login.is_empty() {
        return Err(GhError::InvalidOutput("gh api user returned no login".into()));
    }
    Ok(GhStatus {
        path: path.to_string_lossy().into_owned(),
        hosts,
        login,
    })
}

pub async fn status(
    explicit_path: Option<&Path>,
    workdir: &Path,
    task_id: &str,
) -> Result<GhCapability, GhError> {
    let path = discover_gh_path(explicit_path)?;
    let status = auth_status(&path, workdir).await?;
    Ok(GhCapability {
        status,
        task_id: task_id.to_string(),
        workdir: workdir.to_string_lossy().into_owned(),
    })
}

pub async fn pr_create(
    explicit_path: Option<&Path>,
    workdir: &Path,
    _task_id: &str,
    request: &GhPrCreateRequest,
) -> Result<GhPullRequest, GhError> {
    if request.title.trim().is_empty() {
        return Err(GhError::InvalidOutput("title is required".into()));
    }
    let path = discover_gh_path(explicit_path)?;
    let mut args = vec![
        "pr",
        "create",
        "--title",
        request.title.as_str(),
        "--body",
        request.body.as_str(),
    ];
    if let Some(base) = request.base.as_deref().filter(|value| !value.trim().is_empty()) {
        args.extend(["--base", base]);
    }
    if let Some(head) = request.head.as_deref().filter(|value| !value.trim().is_empty()) {
        args.extend(["--head", head]);
    }
    let output = run_gh(&path, workdir, &args).await?;
    let url = output
        .lines()
        .find(|line| line.starts_with("http"))
        .unwrap_or("")
        .trim();
    if url.is_empty() {
        return Err(GhError::InvalidOutput(
            "gh pr create returned no URL".into(),
        ));
    }
    Ok(GhPullRequest {
        url: url.to_string(),
        number: None,
        repository: None,
        head: request.head.clone(),
        base: request.base.clone(),
    })
}

pub async fn pr_view(
    explicit_path: Option<&Path>,
    workdir: &Path,
    request: &GhPrViewRequest,
) -> Result<serde_json::Value, GhError> {
    let path = discover_gh_path(explicit_path)?;
    let output = run_gh(
        &path,
        workdir,
        &[
            "pr",
            "view",
            &request.number,
            "--json",
            "number,url,headRefName,baseRefName,repository",
        ],
    )
    .await?;
    serde_json::from_str(&output).map_err(|error| GhError::InvalidOutput(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{discover_gh_path, sanitize_output};
    use std::path::Path;

    #[test]
    fn explicit_missing_path_is_rejected_without_path_discovery() {
        assert!(discover_gh_path(Some(Path::new("/definitely/missing/gh"))).is_err());
    }

    #[test]
    fn sanitized_output_does_not_leak_credentials() {
        let output = sanitize_output("host=github.com token=secret\nlogin=alice\n");
        assert_eq!(output, "login=alice");
    }
}
