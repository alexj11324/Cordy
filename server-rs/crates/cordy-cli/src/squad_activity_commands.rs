use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{
    new_api_client, resolve_issue_ref, Cli, Environment, OutputFormat, RunOutput, SquadActivityArgs,
};

pub(super) async fn run_squad_activity(
    cli: &Cli,
    environment: &Environment,
    args: &SquadActivityArgs,
) -> Result<RunOutput> {
    let outcome = args.outcome.as_str();
    if !matches!(outcome, "action" | "no_action" | "failed") {
        bail!("invalid outcome {outcome:?}; valid values: action, no_action, failed");
    }
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let result: Value = client
        .post_json(
            &format!("/api/issues/{issue_id}/squad-evaluated"),
            &serde_json::json!({
                "outcome": outcome,
                "reason": args.reason.as_str(),
            }),
        )
        .await
        .context("record evaluation")?;
    let issue_display = args.issue_id.trim();
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
            OutputFormat::Table => String::new(),
        },
        stderr: format!("Squad evaluation recorded: {outcome} (issue {issue_display})\n"),
    })
}
