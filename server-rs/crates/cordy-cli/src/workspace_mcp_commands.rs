use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::Path;

use super::{
    encoded_path_segment, format_table, new_api_client, resolve_workspace_arg, Cli,
    Environment, OutputFormat, RunOutput, WorkspaceMcpAddArgs, WorkspaceMcpUpdateArgs,
};

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct WorkspaceMcpServer {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) transport: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) enabled: Option<bool>,
}

pub(super) fn format_workspace_mcp_servers(
    servers: &[WorkspaceMcpServer],
    output: OutputFormat,
) -> Result<String> {
    match output {
        OutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(servers)?)),
        OutputFormat::Table if servers.is_empty() => Ok("no MCP servers\n".into()),
        OutputFormat::Table => {
            let mut rows = vec![vec![
                "ID".into(),
                "NAME".into(),
                "TRANSPORT".into(),
                "STATUS".into(),
            ]];
            rows.extend(servers.iter().map(|server| {
                vec![
                    server.id.clone(),
                    server.name.clone(),
                    server.transport.clone(),
                    server.enabled.map_or_else(String::new, |enabled| {
                        if enabled { "enabled" } else { "disabled" }.into()
                    }),
                ]
            }));
            Ok(format_table(&rows))
        }
    }
}

pub(super) async fn run_workspace_mcp_list(
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
    let servers: Vec<WorkspaceMcpServer> = client
        .get_json(&format!("/api/workspaces/{workspace_id}/mcp-servers"))
        .await
        .context("list workspace mcp servers")?;
    Ok(RunOutput {
        stdout: format_workspace_mcp_servers(&servers, output)?,
        stderr: String::new(),
    })
}

pub(super) fn parse_workspace_mcp_server_config(raw: &str) -> Result<Value> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("--server-config: empty input; pass a JSON object");
    }
    let value: Value = serde_json::from_str(raw)
        .map_err(|_| anyhow::anyhow!("--server-config must be a valid JSON object"))?;
    match &value {
        Value::Null => bail!("--server-config must be a JSON object, not null"),
        Value::Object(_) => Ok(value),
        _ => bail!("--server-config must be a JSON object"),
    }
}

pub(super) fn resolve_workspace_mcp_server_config<R: Read>(
    inline: Option<&str>,
    from_stdin: bool,
    file: Option<&Path>,
    environment: &Environment,
    input: &mut R,
) -> Result<Option<Value>> {
    let count = [inline.is_some(), from_stdin, file.is_some()]
        .into_iter()
        .filter(|source| *source)
        .count();
    if count > 1 {
        bail!(
            "--server-config, --server-config-stdin, and --server-config-file are mutually exclusive; pick one"
        );
    }
    let raw = if let Some(inline) = inline {
        inline.into()
    } else if from_stdin {
        let mut bytes = Vec::new();
        input
            .read_to_end(&mut bytes)
            .context("read --server-config-stdin")?;
        let raw = String::from_utf8_lossy(&bytes).into_owned();
        if raw.trim().is_empty() {
            bail!("--server-config-stdin: empty input");
        }
        raw
    } else if let Some(file) = file {
        if file.as_os_str().is_empty() {
            bail!("--server-config-file: path must not be empty");
        }
        let path = if file.is_absolute() {
            file.to_path_buf()
        } else {
            environment.current_dir().join(file)
        };
        let bytes = fs::read(&path).context("read --server-config-file")?;
        let raw = String::from_utf8_lossy(&bytes).into_owned();
        if raw.trim().is_empty() {
            bail!(
                "--server-config-file {:?}: empty contents",
                file.to_string_lossy()
            );
        }
        raw
    } else {
        return Ok(None);
    };
    parse_workspace_mcp_server_config(&raw).map(Some)
}

pub(super) async fn run_workspace_mcp_add<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &WorkspaceMcpAddArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let server_name = args.server_name.trim();
    if server_name.is_empty() {
        bail!("server name must not be empty");
    }
    let workspace_id = resolve_workspace_arg(cli, environment, args.workspace.as_deref()).await?;
    if workspace_id.is_empty() {
        bail!(
            "workspace ID is required: pass an id/slug/prefix as argument or set CORDY_WORKSPACE_ID"
        );
    }
    let config = resolve_workspace_mcp_server_config(
        args.server_config.as_deref(),
        args.server_config_stdin,
        args.server_config_file.as_deref(),
        environment,
        input,
    )?
    .context(
        "one of --server-config, --server-config-stdin, or --server-config-file is required",
    )?;
    let client = new_api_client(cli, environment)?;
    let server: WorkspaceMcpServer = client
        .post_json(
            &format!("/api/workspaces/{workspace_id}/mcp-servers"),
            &serde_json::json!({"name":server_name,"config":config}),
        )
        .await
        .context("add workspace mcp server")?;
    Ok(RunOutput {
        stdout: format_workspace_mcp_servers(std::slice::from_ref(&server), args.output)?,
        stderr: String::new(),
    })
}

pub(super) async fn run_workspace_mcp_update<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &WorkspaceMcpUpdateArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let server_id = args.server_id.trim();
    if server_id.is_empty() {
        bail!("server ID must not be empty");
    }
    let workspace_id = resolve_workspace_arg(cli, environment, args.workspace.as_deref()).await?;
    if workspace_id.is_empty() {
        bail!(
            "workspace ID is required: pass an id/slug/prefix as argument or set CORDY_WORKSPACE_ID"
        );
    }
    let mut body = serde_json::Map::new();
    if let Some(name) = &args.name {
        body.insert("name".into(), Value::String(name.trim().into()));
    }
    if let Some(config) = resolve_workspace_mcp_server_config(
        args.server_config.as_deref(),
        args.server_config_stdin,
        args.server_config_file.as_deref(),
        environment,
        input,
    )? {
        body.insert("config".into(), config);
    }
    if body.is_empty() {
        bail!(
            "nothing to update; pass --name and/or one of --server-config, --server-config-stdin, --server-config-file"
        );
    }
    let client = new_api_client(cli, environment)?;
    let server: WorkspaceMcpServer = client
        .put_json(
            &format!(
                "/api/workspaces/{workspace_id}/mcp-servers/{}",
                encoded_path_segment(server_id)
            ),
            &body,
        )
        .await
        .context("update workspace mcp server")?;
    Ok(RunOutput {
        stdout: format_workspace_mcp_servers(std::slice::from_ref(&server), args.output)?,
        stderr: String::new(),
    })
}

pub(super) async fn run_workspace_mcp_remove(
    cli: &Cli,
    environment: &Environment,
    server_id: &str,
    workspace: Option<&str>,
    _output: OutputFormat,
) -> Result<RunOutput> {
    let server_id = server_id.trim();
    if server_id.is_empty() {
        bail!("server ID must not be empty");
    }
    let workspace_id = resolve_workspace_arg(cli, environment, workspace).await?;
    if workspace_id.is_empty() {
        bail!(
            "workspace ID is required: pass an id/slug/prefix as argument or set CORDY_WORKSPACE_ID"
        );
    }
    let client = new_api_client(cli, environment)?;
    client
        .delete(&format!(
            "/api/workspaces/{workspace_id}/mcp-servers/{}",
            encoded_path_segment(server_id)
        ))
        .await
        .context("remove workspace mcp server")?;
    Ok(RunOutput {
        stdout: format!("removed MCP server {server_id}\n"),
        stderr: String::new(),
    })
}
