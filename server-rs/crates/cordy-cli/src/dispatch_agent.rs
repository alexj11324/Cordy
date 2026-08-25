//! Agent command dispatch.
//!
//! Keeping this branch group separate prevents the root dispatcher from
//! mixing agent lifecycle, skills, environment, MCP, and copy policies with
//! unrelated command domains.

use std::io::Read;

use super::*;

pub(super) async fn run_agent_command<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &AgentArgs,
    input: &mut R,
) -> Result<RunOutput> {
    match args {
        AgentArgs {
            command:
                AgentCommand::List {
                    output,
                    include_archived,
                },
        } => run_agent_list(cli, environment, *output, *include_archived).await,
        AgentArgs {
            command: AgentCommand::Get { id, output },
        } => run_agent_get(cli, environment, id, *output).await,
        AgentArgs {
            command: AgentCommand::Create(args),
        } => run_agent_create(cli, environment, args, input).await,
        AgentArgs {
            command: AgentCommand::Update(args),
        } => run_agent_update(cli, environment, args, input).await,
        AgentArgs {
            command: AgentCommand::Archive { id, output },
        } => run_agent_lifecycle(cli, environment, id, "archive", "archived", *output).await,
        AgentArgs {
            command: AgentCommand::Restore { id, output },
        } => run_agent_lifecycle(cli, environment, id, "restore", "restored", *output).await,
        AgentArgs {
            command: AgentCommand::Tasks { id, output },
        } => run_agent_tasks(cli, environment, id, *output).await,
        AgentArgs {
            command: AgentCommand::Avatar { id, file, output },
        } => run_agent_avatar(cli, environment, id, file.as_deref(), *output).await,
        AgentArgs {
            command:
                AgentCommand::Skills(AgentSkillsArgs {
                    command: AgentSkillsCommand::List { agent_id, output },
                }),
        } => run_agent_skills_list(cli, environment, agent_id, *output).await,
        AgentArgs {
            command:
                AgentCommand::Skills(AgentSkillsArgs {
                    command: AgentSkillsCommand::Set(args),
                }),
        } => run_agent_skills_mutation(cli, environment, args, false).await,
        AgentArgs {
            command:
                AgentCommand::Skills(AgentSkillsArgs {
                    command: AgentSkillsCommand::Add(args),
                }),
        } => run_agent_skills_mutation(cli, environment, args, true).await,
        AgentArgs {
            command:
                AgentCommand::Env(AgentEnvArgs {
                    command: AgentEnvCommand::Get { agent_id, output },
                }),
        } => run_agent_env_get(cli, environment, agent_id, *output).await,
        AgentArgs {
            command:
                AgentCommand::Env(AgentEnvArgs {
                    command: AgentEnvCommand::Set(args),
                }),
        } => run_agent_env_set(cli, environment, args, input).await,
        AgentArgs {
            command:
                AgentCommand::Mcp(AgentMcpArgs {
                    command: AgentMcpCommand::List(args),
                }),
        } => run_agent_mcp_list(cli, environment, args).await,
        AgentArgs {
            command:
                AgentCommand::Mcp(AgentMcpArgs {
                    command: AgentMcpCommand::Add(args),
                }),
        } => run_agent_mcp_mutation(cli, environment, args, AgentMcpAction::Add).await,
        AgentArgs {
            command:
                AgentCommand::Mcp(AgentMcpArgs {
                    command: AgentMcpCommand::Enable(args),
                }),
        } => run_agent_mcp_mutation(cli, environment, args, AgentMcpAction::Enable).await,
        AgentArgs {
            command:
                AgentCommand::Mcp(AgentMcpArgs {
                    command: AgentMcpCommand::Disable(args),
                }),
        } => run_agent_mcp_mutation(cli, environment, args, AgentMcpAction::Disable).await,
        AgentArgs {
            command:
                AgentCommand::Mcp(AgentMcpArgs {
                    command: AgentMcpCommand::Remove(args),
                }),
        } => run_agent_mcp_mutation(cli, environment, args, AgentMcpAction::Remove).await,
        AgentArgs {
            command: AgentCommand::Copy(args),
        } => run_agent_copy(cli, environment, args, input).await,
    }
}
