use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{value_string, Environment, RunOutput};

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
