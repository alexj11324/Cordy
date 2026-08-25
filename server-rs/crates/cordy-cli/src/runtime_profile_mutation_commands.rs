use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{
    new_api_client, output_runtime_profiles, required_workspace_id, runtime_profiles_path, Cli,
    Environment, RunOutput, RuntimeProfileCreateArgs, RuntimeProfileUpdateArgs,
};

const RUNTIME_PROTOCOL_FAMILIES: &[&str] = &[
    "claude",
    "codebuddy",
    "codex",
    "copilot",
    "opencode",
    "deveco",
    "openclaw",
    "hermes",
    "pi",
    "cursor",
    "kimi",
    "reasonix",
    "dsh",
    "kiro",
    "antigravity",
    "qoder",
    "qoderclicn",
    "traecli",
    "grok",
    "qwen",
    "qwenpaw",
    "mcode",
    "dim",
];

pub(super) async fn run_runtime_profile_create(
    cli: &Cli,
    environment: &Environment,
    args: &RuntimeProfileCreateArgs,
) -> Result<RunOutput> {
    let family = args
        .protocol_family
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .context("--protocol-family is required")?;
    let command_name = args
        .command_name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .context("--command-name is required")?;
    let display_name = args
        .display_name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .context("--display-name is required")?;
    if !RUNTIME_PROTOCOL_FAMILIES.contains(&family) {
        bail!(
            "invalid --protocol-family {:?}: must be one of {}",
            family,
            RUNTIME_PROTOCOL_FAMILIES.join(", ")
        );
    }
    let client = new_api_client(cli, environment)?;
    let workspace_id = required_workspace_id(cli, environment)?;
    let mut body = serde_json::Map::from_iter([
        ("display_name".into(), Value::String(display_name.into())),
        ("protocol_family".into(), Value::String(family.into())),
        ("command_name".into(), Value::String(command_name.into())),
    ]);
    if !args.description.is_empty() {
        body.insert(
            "description".into(),
            Value::String(args.description.clone()),
        );
    }
    let profile: Value = client
        .post_json(&runtime_profiles_path(&workspace_id), &body)
        .await
        .context("create runtime profile")?;
    output_runtime_profiles(&[profile], args.output, true)
}

pub(super) async fn run_runtime_profile_update(
    cli: &Cli,
    environment: &Environment,
    args: &RuntimeProfileUpdateArgs,
) -> Result<RunOutput> {
    let mut body = serde_json::Map::new();
    for (key, value) in [
        ("display_name", &args.display_name),
        ("command_name", &args.command_name),
        ("description", &args.description),
    ] {
        if let Some(value) = value {
            body.insert(key.into(), Value::String(value.clone()));
        }
    }
    if let Some(enabled) = args.enabled {
        body.insert("enabled".into(), Value::Bool(enabled));
    }
    if body.is_empty() {
        bail!("no fields to update: pass at least one of --display-name, --command-name, --description, --enabled");
    }
    let client = new_api_client(cli, environment)?;
    let workspace_id = required_workspace_id(cli, environment)?;
    let profile: Value = client
        .patch_json(
            &format!(
                "{}/{}",
                runtime_profiles_path(&workspace_id),
                args.profile_id
            ),
            &body,
        )
        .await
        .context("update runtime profile")?;
    output_runtime_profiles(&[profile], args.output, true)
}

pub(super) async fn run_runtime_profile_delete(
    cli: &Cli,
    environment: &Environment,
    profile_id: &str,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = required_workspace_id(cli, environment)?;
    let path = format!("{}/{profile_id}", runtime_profiles_path(&workspace_id));
    if let Err(error) = client.delete(&path).await {
        if error
            .downcast_ref::<super::HttpError>()
            .is_some_and(|http| http.status_code == 409)
        {
            let message = error
                .downcast_ref::<super::HttpError>()
                .map(|http| http.body.trim())
                .filter(|body| !body.is_empty())
                .unwrap_or("profile still has active agents bound to it");
            bail!("cannot delete runtime profile {profile_id}: {message}");
        }
        return Err(error).context("delete runtime profile");
    }
    Ok(RunOutput {
        stdout: format!("Deleted runtime profile {profile_id}\n"),
        stderr: String::new(),
    })
}
