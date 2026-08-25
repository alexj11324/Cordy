//! Workspace, member, and MCP command dispatch.
//!
//! Workspace-scoped routing stays together so selection, membership, and MCP
//! mutations preserve their existing scope and input-forwarding semantics.

use std::io::Read;

use super::*;

pub(super) async fn run_workspace_command<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &WorkspaceArgs,
    input: &mut R,
) -> Result<RunOutput> {
    match args {
        WorkspaceArgs {
            command: WorkspaceCommand::List { output, full_id },
        } => run_workspace_list(cli, environment, *output, *full_id).await,
        WorkspaceArgs {
            command: WorkspaceCommand::Get { workspace, output },
        } => run_workspace_get(cli, environment, workspace.as_deref(), *output).await,
        WorkspaceArgs {
            command: WorkspaceCommand::Create(args),
        } => run_workspace_create(cli, environment, args, input).await,
        WorkspaceArgs {
            command: WorkspaceCommand::Update(args),
        } => run_workspace_update(cli, environment, args, input).await,
        WorkspaceArgs {
            command: WorkspaceCommand::Switch { workspace },
        } => run_workspace_switch(cli, environment, workspace).await,
        WorkspaceArgs {
            command:
                WorkspaceCommand::Member(WorkspaceMemberArgs {
                    command: WorkspaceMemberCommand::List { workspace, output },
                }),
        } => run_workspace_member_list(cli, environment, workspace.as_deref(), *output).await,
        WorkspaceArgs {
            command:
                WorkspaceCommand::Member(WorkspaceMemberArgs {
                    command: WorkspaceMemberCommand::Invite(args),
                }),
        } => run_workspace_member_invite(cli, environment, args).await,
        WorkspaceArgs {
            command:
                WorkspaceCommand::Mcp(WorkspaceMcpArgs {
                    command: WorkspaceMcpCommand::List { workspace, output },
                }),
        } => run_workspace_mcp_list(cli, environment, workspace.as_deref(), *output).await,
        WorkspaceArgs {
            command:
                WorkspaceCommand::Mcp(WorkspaceMcpArgs {
                    command: WorkspaceMcpCommand::Add(args),
                }),
        } => run_workspace_mcp_add(cli, environment, args, input).await,
        WorkspaceArgs {
            command:
                WorkspaceCommand::Mcp(WorkspaceMcpArgs {
                    command: WorkspaceMcpCommand::Update(args),
                }),
        } => run_workspace_mcp_update(cli, environment, args, input).await,
        WorkspaceArgs {
            command:
                WorkspaceCommand::Mcp(WorkspaceMcpArgs {
                    command:
                        WorkspaceMcpCommand::Remove {
                            server_id,
                            workspace,
                            output,
                        },
                }),
        } => {
            run_workspace_mcp_remove(cli, environment, server_id, workspace.as_deref(), *output)
                .await
        }
    }
}
