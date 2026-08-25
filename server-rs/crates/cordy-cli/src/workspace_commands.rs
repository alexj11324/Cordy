use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Write;
use std::fs;
use std::io::Read;
use std::path::Path;

use super::{
    compact_uuid, display_id, ensure_file_within_workdir, format_table, is_canonical_uuid,
    new_api_client, new_unscoped_api_client, new_unscoped_authenticated_api_client,
    normalize_uuid_prefix, require_human_local_command, resolve_current_workspace_id,
    trim_one_trailing_newline, truncate_text, unescape_backslash_escapes, value_string, Cli,
    CreateWorkspaceArgs, Environment, OutputFormat, RunOutput, UpdateWorkspaceArgs,
};

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct WorkspaceSummary {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) slug: String,
}

pub(super) async fn run_workspace_list(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
    full_id: bool,
) -> Result<RunOutput> {
    let workspaces = fetch_workspaces(cli, environment).await?;
    if output == OutputFormat::Json {
        return Ok(RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(&workspaces)?),
            stderr: String::new(),
        });
    }
    if workspaces.is_empty() {
        return Ok(RunOutput {
            stdout: String::new(),
            stderr: "No workspaces found.\n".into(),
        });
    }

    let current_id = resolve_current_workspace_id(cli, environment);
    let stdout = format_workspace_table(&workspaces, &current_id, full_id);
    let current_hint = if current_id.is_empty() {
        "\nNo default workspace set. Use 'cordy workspace switch <id|slug|prefix>' to pick one.\n"
    } else {
        "\n* = current default workspace (use 'cordy workspace switch <id|slug|prefix>' to change)\n"
    };
    Ok(RunOutput {
        stdout,
        stderr: format!(
            "{current_hint}Tip: pass the ID column, SLUG, or full UUID (--full-id) to 'workspace get/update/switch'.\n"
        ),
    })
}

async fn fetch_workspaces(cli: &Cli, environment: &Environment) -> Result<Vec<WorkspaceSummary>> {
    let client = new_unscoped_authenticated_api_client(cli, environment)?;
    client
        .get_json("/api/workspaces")
        .await
        .context("list workspaces")
}

pub(super) async fn run_workspace_get(
    cli: &Cli,
    environment: &Environment,
    workspace: Option<&str>,
    output: OutputFormat,
) -> Result<RunOutput> {
    let workspace_id = resolve_workspace_arg(cli, environment, workspace).await?;
    if workspace_id.is_empty() {
        bail!(
            "workspace ID is required: pass an id/slug/prefix as argument or set CORDY_WORKSPACE_ID"
        );
    }
    let client = new_api_client(cli, environment)?;
    let workspace: Value = client
        .get_json(&format!("/api/workspaces/{workspace_id}"))
        .await
        .context("get workspace")?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&workspace)?),
            OutputFormat::Table => format_workspace_details_table(&workspace),
        },
        stderr: String::new(),
    })
}

#[derive(Debug, Serialize)]
struct CreateWorkspaceBody {
    name: String,
    slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    issue_prefix: Option<String>,
}

pub(super) async fn run_workspace_create<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &CreateWorkspaceArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let body = build_workspace_create_body(args, input)?;
    let client = new_unscoped_api_client(cli, environment)?;
    let workspace: Value = client
        .post_json("/api/workspaces", &body)
        .await
        .context("create workspace")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&workspace)?),
            OutputFormat::Table => format_workspace_details_table(&workspace),
        },
        stderr: String::new(),
    })
}

pub(super) fn build_workspace_create_body<R: Read>(
    args: &CreateWorkspaceArgs,
    input: &mut R,
) -> Result<impl Serialize> {
    let name = args.name.as_deref().unwrap_or_default();
    if name.trim().is_empty() {
        bail!("--name is required");
    }
    let slug = args.slug.as_deref().unwrap_or_default();
    if slug.trim().is_empty() {
        bail!("--slug is required");
    }
    if args.description_stdin && args.context_stdin {
        bail!(
            "--description-stdin and --context-stdin cannot be combined; a single stdin cannot feed both fields — pass one of them inline"
        );
    }
    let description = resolve_optional_text_input(
        args.description.as_deref(),
        args.description_stdin,
        "description",
        input,
    )?;
    let context = resolve_optional_text_input(
        args.context.as_deref(),
        args.context_stdin,
        "context",
        input,
    )?;
    let issue_prefix = args
        .issue_prefix
        .as_ref()
        .map(|prefix| {
            if prefix.trim().is_empty() {
                bail!("--issue-prefix cannot be empty; omit it to use the server-generated prefix");
            }
            Ok(prefix.clone())
        })
        .transpose()?;
    Ok(CreateWorkspaceBody {
        name: name.into(),
        slug: slug.into(),
        description,
        context,
        issue_prefix,
    })
}

fn resolve_optional_text_input<R: Read>(
    inline: Option<&str>,
    use_stdin: bool,
    field: &str,
    input: &mut R,
) -> Result<Option<String>> {
    if use_stdin && inline.is_some_and(|value| !value.is_empty()) {
        bail!("--{field} and --{field}-stdin are mutually exclusive");
    }
    if use_stdin {
        let mut bytes = Vec::new();
        input
            .read_to_end(&mut bytes)
            .with_context(|| format!("read stdin for --{field}-stdin"))?;
        let body = trim_one_trailing_newline(String::from_utf8_lossy(&bytes).into_owned());
        if body.is_empty() {
            bail!("stdin content for --{field}-stdin is empty");
        }
        return Ok(Some(body));
    }
    Ok(inline.map(unescape_backslash_escapes))
}

pub(super) async fn run_workspace_update<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &UpdateWorkspaceArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let workspace_id = resolve_workspace_arg(cli, environment, args.workspace.as_deref()).await?;
    if workspace_id.is_empty() {
        bail!(
            "workspace ID is required: pass an id/slug/prefix as argument or set CORDY_WORKSPACE_ID"
        );
    }
    let body = build_workspace_update_body(args, environment, input)?;
    if body.is_empty() {
        bail!("no fields to update; use --name, --description, --context, or --issue-prefix");
    }
    let client = new_api_client(cli, environment)?;
    let workspace: Value = client
        .patch_json(&format!("/api/workspaces/{workspace_id}"), &body)
        .await
        .context("update workspace")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&workspace)?),
            OutputFormat::Table => format_workspace_details_table(&workspace),
        },
        stderr: String::new(),
    })
}

pub(super) async fn run_workspace_switch(
    cli: &Cli,
    environment: &Environment,
    workspace: &str,
) -> Result<RunOutput> {
    require_human_local_command(environment, "workspace switch")?;
    let target = workspace.trim();
    if target.is_empty() {
        bail!("workspace id, slug, or id prefix is required");
    }
    let workspaces = fetch_workspaces(cli, environment).await?;
    let workspace = resolve_workspace_reference(&workspaces, target)?;
    environment.set_profile_value(
        &cli.profile,
        "workspace_id",
        Some(Value::String(workspace.id.clone())),
    )?;
    Ok(RunOutput {
        stdout: format!(
            "Switched to workspace: {} ({})\n",
            workspace.name, workspace.id
        ),
        stderr: String::new(),
    })
}

pub(super) fn build_workspace_update_body<R: Read>(
    args: &UpdateWorkspaceArgs,
    environment: &Environment,
    input: &mut R,
) -> Result<serde_json::Map<String, Value>> {
    if args.description_stdin && args.context_stdin {
        bail!(
            "--description-stdin and --context-stdin cannot be combined; a single stdin cannot feed both fields — pass one of them inline or by file"
        );
    }
    let mut body = serde_json::Map::new();
    if let Some(name) = &args.name {
        body.insert("name".into(), Value::String(name.clone()));
    }
    if let Some(description) = resolve_update_text_input(
        args.description.as_deref(),
        args.description_stdin,
        args.description_file.as_deref(),
        args.allow_external_file,
        "description",
        environment,
        input,
    )? {
        body.insert("description".into(), Value::String(description));
    }
    if let Some(context) = resolve_update_text_input(
        args.context.as_deref(),
        args.context_stdin,
        args.context_file.as_deref(),
        args.allow_external_file,
        "context",
        environment,
        input,
    )? {
        body.insert("context".into(), Value::String(context));
    }
    if let Some(issue_prefix) = &args.issue_prefix {
        if issue_prefix.trim().is_empty() {
            bail!("--issue-prefix cannot be empty; clearing the prefix is not supported");
        }
        body.insert("issue_prefix".into(), Value::String(issue_prefix.clone()));
    }
    Ok(body)
}

#[allow(clippy::too_many_arguments)]
fn resolve_update_text_input<R: Read>(
    inline: Option<&str>,
    use_stdin: bool,
    file: Option<&Path>,
    allow_external_file: bool,
    field: &str,
    environment: &Environment,
    input: &mut R,
) -> Result<Option<String>> {
    let sources = [use_stdin, inline.is_some(), file.is_some()]
        .into_iter()
        .filter(|source| *source)
        .count();
    if sources > 1 {
        bail!("--{field}, --{field}-stdin, and --{field}-file are mutually exclusive");
    }
    if use_stdin {
        let mut bytes = Vec::new();
        input
            .read_to_end(&mut bytes)
            .with_context(|| format!("read stdin for --{field}-stdin"))?;
        let body = trim_one_trailing_newline(String::from_utf8_lossy(&bytes).into_owned());
        if body.is_empty() {
            bail!("stdin content for --{field}-stdin is empty");
        }
        return Ok(Some(body));
    }
    if let Some(path) = file {
        ensure_file_within_workdir(path, environment.current_dir(), allow_external_file, field)?;
        let read_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            environment.current_dir().join(path)
        };
        let bytes = fs::read(read_path).with_context(|| format!("read file for --{field}-file"))?;
        let body = trim_one_trailing_newline(String::from_utf8_lossy(&bytes).into_owned());
        if body.is_empty() {
            bail!("file content for --{field}-file is empty");
        }
        return Ok(Some(body));
    }
    Ok(inline.map(unescape_backslash_escapes))
}

pub(super) async fn resolve_workspace_arg(
    cli: &Cli,
    environment: &Environment,
    workspace: Option<&str>,
) -> Result<String> {
    let Some(workspace) = workspace else {
        return Ok(resolve_current_workspace_id(cli, environment));
    };
    let target = workspace.trim();
    if target.is_empty() {
        bail!("workspace id, slug, or id prefix is required");
    }
    if is_canonical_uuid(target) {
        return Ok(target.into());
    }
    let workspaces = fetch_workspaces(cli, environment).await?;
    Ok(resolve_workspace_reference(&workspaces, target)?.id.clone())
}

pub(super) fn resolve_workspace_reference<'a>(
    workspaces: &'a [WorkspaceSummary],
    target: &str,
) -> Result<&'a WorkspaceSummary> {
    let target = target.trim();
    if target.is_empty() {
        bail!("workspace id, slug, or id prefix is required");
    }
    if let Some(workspace) = workspaces
        .iter()
        .find(|workspace| workspace.id.eq_ignore_ascii_case(target))
    {
        return Ok(workspace);
    }
    if let Some(workspace) = workspaces
        .iter()
        .find(|workspace| !workspace.slug.is_empty() && workspace.slug.eq_ignore_ascii_case(target))
    {
        return Ok(workspace);
    }
    if let Some(prefix) = normalize_uuid_prefix(target) {
        let matches: Vec<_> = workspaces
            .iter()
            .filter(|workspace| compact_uuid(&workspace.id).starts_with(&prefix))
            .collect();
        match matches.as_slice() {
            [workspace] => return Ok(workspace),
            [_, _, ..] => {
                let details = matches
                    .iter()
                    .map(|workspace| {
                        let label = if workspace.slug.is_empty() {
                            workspace.name.clone()
                        } else {
                            format!("{} ({})", workspace.name, workspace.slug)
                        };
                        format!("  {}  {label}", workspace.id)
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                bail!(
                    "ambiguous workspace id prefix {target:?}; matches:\n{details}\nUse more characters, the slug, or the full UUID"
                );
            }
            _ => {}
        }
    }
    bail!(
        "workspace {target:?} not found or you do not have access; run 'cordy workspace list' to see options"
    )
}

pub(super) fn format_workspace_details_table(workspace: &Value) -> String {
    let description = truncate_text(&value_string(workspace, "description"), 60);
    let context = truncate_text(&value_string(workspace, "context"), 60);
    format_table(&[
        vec![
            "ID".into(),
            "NAME".into(),
            "SLUG".into(),
            "DESCRIPTION".into(),
            "CONTEXT".into(),
        ],
        vec![
            value_string(workspace, "id"),
            value_string(workspace, "name"),
            value_string(workspace, "slug"),
            description,
            context,
        ],
    ])
}

pub(super) fn format_workspace_table(
    workspaces: &[WorkspaceSummary],
    current_id: &str,
    full_id: bool,
) -> String {
    let mut rows = Vec::with_capacity(workspaces.len() + 1);
    rows.push([String::new(), "ID".into(), "NAME".into(), "SLUG".into()]);
    rows.extend(workspaces.iter().map(|workspace| {
        [
            (if workspace.id == current_id { "*" } else { " " }).into(),
            display_id(&workspace.id, full_id),
            workspace.name.clone(),
            workspace.slug.clone(),
        ]
    }));
    let widths: [usize; 3] = std::array::from_fn(|column| {
        rows.iter()
            .map(|row| row[column].chars().count())
            .max()
            .unwrap_or_default()
            + 2
    });
    let mut output = String::new();
    for row in rows {
        let _ = writeln!(
            output,
            "{:<marker_width$}{:<id_width$}{:<name_width$}{}",
            row[0],
            row[1],
            row[2],
            row[3],
            marker_width = widths[0],
            id_width = widths[1],
            name_width = widths[2]
        );
    }
    output
}
