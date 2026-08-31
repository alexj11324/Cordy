//! Local GitHub CLI commands. These are task-scoped calls to the daemon,
//! never direct access to a user's gh credentials from the CLI process.

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use serde_json::Value;

use super::{Cli, Environment, OutputFormat, RunOutput};

#[derive(Debug, Args)]
pub(super) struct GithubArgs {
    #[command(subcommand)]
    pub(super) command: GithubCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum GithubCommand {
    #[command(about = "Show the authenticated local GitHub CLI status")]
    Status {
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
    #[command(about = "Create or inspect a pull request through the local daemon")]
    Pr(GithubPrArgs),
}

#[derive(Debug, Args)]
pub(super) struct GithubPrArgs {
    #[command(subcommand)]
    pub(super) command: GithubPrCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum GithubPrCommand {
    #[command(about = "Create a pull request")]
    Create(GithubPrCreateArgs),
    #[command(about = "View a pull request")]
    View(GithubPrViewArgs),
}

#[derive(Debug, Args)]
pub(super) struct GithubPrCreateArgs {
    #[arg(long, help = "Pull request title")]
    pub(super) title: String,
    #[arg(long, default_value = "", help = "Pull request body")]
    pub(super) body: String,
    #[arg(long)]
    pub(super) base: Option<String>,
    #[arg(long)]
    pub(super) head: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct GithubPrViewArgs {
    #[arg(value_name = "NUMBER")]
    pub(super) number: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

fn daemon_context(environment: &Environment) -> Result<(String, String)> {
    let port = environment
        .trimmed("PATCHBAY_DAEMON_PORT")
        .context("PATCHBAY_DAEMON_PORT is required inside an agent task")?;
    let token = environment
        .trimmed("PATCHBAY_TOKEN")
        .context("PATCHBAY_TOKEN is required for local daemon capabilities")?;
    let _task_id = environment
        .trimmed("PATCHBAY_TASK_ID")
        .context("PATCHBAY_TASK_ID is required for local daemon capabilities")?;
    Ok((format!("http://127.0.0.1:{port}"), token.to_string()))
}

async fn call_daemon(
    environment: &Environment,
    path: &str,
    body: Option<Value>,
) -> Result<Value> {
    let (base, token) = daemon_context(environment)?;
    let client = reqwest::Client::new();
    let mut request = client.post(format!("{base}{path}")).bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.context("connect to local daemon")?;
    let status = response.status();
    let text = response.text().await.context("read daemon response")?;
    if !status.is_success() {
        bail!("GitHub capability failed: {text}");
    }
    serde_json::from_str(&text).context("parse daemon response")
}

pub(super) async fn run_github_status(
    _cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
) -> Result<RunOutput> {
    let value = call_daemon(environment, "/github/status", None).await?;
    render(value, output)
}

pub(super) async fn run_github_pr_create(
    _cli: &Cli,
    environment: &Environment,
    args: &GithubPrCreateArgs,
) -> Result<RunOutput> {
    if args.title.trim().is_empty() {
        bail!("--title is required");
    }
    let value = call_daemon(
        environment,
        "/github/pr/create",
        Some(serde_json::json!({
            "title": args.title,
            "body": args.body,
            "base": args.base,
            "head": args.head,
        })),
    )
    .await?;
    render(value, args.output)
}

pub(super) async fn run_github_pr_view(
    _cli: &Cli,
    environment: &Environment,
    args: &GithubPrViewArgs,
) -> Result<RunOutput> {
    if args.number.trim().is_empty() {
        bail!("pull request number is required");
    }
    let value = call_daemon(
        environment,
        "/github/pr/view",
        Some(serde_json::json!({ "number": args.number })),
    )
    .await?;
    render(value, args.output)
}

fn render(value: Value, output: OutputFormat) -> Result<RunOutput> {
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&value)?),
        OutputFormat::Table => format!("{}\n", serde_json::to_string_pretty(&value)?),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}
