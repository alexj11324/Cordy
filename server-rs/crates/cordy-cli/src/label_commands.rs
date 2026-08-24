use anyhow::{bail, Context, Result};
use serde_json::Value;
use url::form_urlencoded;

use super::{
    display_id, format_table, issue_labels, new_api_client, resolve_current_workspace_id,
    resolve_label_id, resolve_label_reference, value_string, Cli, Environment, LabelCreateArgs,
    LabelUpdateArgs, OutputFormat, RunOutput,
};

pub(super) fn format_label_table(labels: &[Value], full_id: bool) -> String {
    let mut rows = vec![vec!["ID".into(), "NAME".into(), "COLOR".into()]];
    rows.extend(labels.iter().map(|label| {
        vec![
            display_id(&value_string(label, "id"), full_id),
            value_string(label, "name"),
            value_string(label, "color"),
        ]
    }));
    format_table(&rows)
}

pub(super) fn format_workspace_label_table(labels: &[Value], full_id: bool) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "NAME".into(),
        "COLOR".into(),
        "CREATED".into(),
    ]];
    rows.extend(labels.iter().map(|label| {
        vec![
            display_id(&value_string(label, "id"), full_id),
            value_string(label, "name"),
            value_string(label, "color"),
            value_string(label, "created_at").chars().take(10).collect(),
        ]
    }));
    format_table(&rows)
}

pub(super) fn format_label_result(
    label: &Value,
    output: OutputFormat,
    include_created: bool,
) -> Result<String> {
    match output {
        OutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(label)?)),
        OutputFormat::Table if include_created => Ok(format_workspace_label_table(
            std::slice::from_ref(label),
            true,
        )),
        OutputFormat::Table => Ok(format_label_table(std::slice::from_ref(label), true)),
    }
}

pub(super) async fn run_label_list(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
    full_id: bool,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let path = if workspace_id.is_empty() {
        "/api/labels".into()
    } else {
        format!(
            "/api/labels?workspace_id={}",
            form_urlencoded::byte_serialize(workspace_id.as_bytes()).collect::<String>()
        )
    };
    let result: Value = client.get_json(&path).await.context("list labels")?;
    let labels = issue_labels(&result);
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(labels)?),
            OutputFormat::Table => format_workspace_label_table(labels, full_id),
        },
        stderr: String::new(),
    })
}

pub(super) async fn run_label_get(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let label_id = resolve_label_id(&client, &workspace_id, id)
        .await
        .context("resolve label")?;
    let label: Value = client
        .get_json(&format!("/api/labels/{label_id}"))
        .await
        .context("get label")?;
    Ok(RunOutput {
        stdout: format_label_result(&label, output, true)?,
        stderr: String::new(),
    })
}

pub(super) async fn run_label_create(
    cli: &Cli,
    environment: &Environment,
    args: &LabelCreateArgs,
) -> Result<RunOutput> {
    let name = args
        .name
        .as_deref()
        .filter(|name| !name.is_empty())
        .context("--name is required")?;
    let color = args
        .color
        .as_deref()
        .filter(|color| !color.is_empty())
        .context("--color is required (e.g. #3b82f6)")?;
    let client = new_api_client(cli, environment)?;
    let label: Value = client
        .post_json(
            "/api/labels",
            &serde_json::json!({"name":name,"color":color}),
        )
        .await
        .context("create label")?;
    Ok(RunOutput {
        stdout: format_label_result(&label, args.output, false)?,
        stderr: String::new(),
    })
}

pub(super) async fn run_label_update(
    cli: &Cli,
    environment: &Environment,
    args: &LabelUpdateArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let label_id = resolve_label_id(&client, &workspace_id, &args.id)
        .await
        .context("resolve label")?;
    let mut body = serde_json::Map::new();
    if let Some(name) = args.name.as_deref().filter(|name| !name.is_empty()) {
        body.insert("name".into(), Value::String(name.into()));
    }
    if let Some(color) = args.color.as_deref().filter(|color| !color.is_empty()) {
        body.insert("color".into(), Value::String(color.into()));
    }
    if body.is_empty() {
        bail!("nothing to update — provide --name and/or --color");
    }
    let label: Value = client
        .put_json(&format!("/api/labels/{label_id}"), &body)
        .await
        .context("update label")?;
    Ok(RunOutput {
        stdout: format_label_result(&label, args.output, false)?,
        stderr: String::new(),
    })
}

pub(super) async fn run_label_delete(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (label_id, display) = resolve_label_reference(&client, &workspace_id, id)
        .await
        .context("resolve label")?;
    client
        .delete(&format!("/api/labels/{label_id}"))
        .await
        .context("delete label")?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!(
                "{}\n",
                serde_json::to_string_pretty(&serde_json::json!({"id":label_id,"deleted":true}))?
            ),
            OutputFormat::Table => format!("Label {display} deleted.\n"),
        },
        stderr: String::new(),
    })
}
