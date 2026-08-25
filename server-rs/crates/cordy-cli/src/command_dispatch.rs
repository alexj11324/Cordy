use std::io::Read;

use super::dispatch_agent::run_agent_command;
use super::dispatch_attachment::run_attachment_command;
use super::dispatch_auth::{run_auth_command, run_login_command};
use super::dispatch_autopilot::run_autopilot_command;
use super::dispatch_chat::run_chat_command;
use super::dispatch_config::run_config_command;
use super::dispatch_issue::run_issue_command;
use super::dispatch_label::run_label_command;
use super::dispatch_project::run_project_command;
use super::dispatch_property::run_property_command;
use super::dispatch_skill::run_skill_command;
use super::dispatch_squad::run_squad_command;
use super::dispatch_user::run_user_command;
use super::dispatch_workspace::run_workspace_command;
use super::*;

pub(super) async fn run_with_input<R: Read>(
    cli: &Cli,
    environment: &Environment,
    input: &mut R,
) -> Result<RunOutput> {
    match &cli.command {
        Command::Agent(args) => run_agent_command(cli, environment, args, input).await,
        Command::Skill(args) => run_skill_command(cli, environment, args, input).await,
        Command::Autopilot(args) => run_autopilot_command(cli, environment, args, input).await,
        Command::Issue(args) => run_issue_command(cli, environment, args, input).await,
        Command::Auth(args) => run_auth_command(cli, environment, args).await,
        Command::Login(args) => run_login_command(cli, environment, args).await,
        Command::Config(args) => run_config_command(cli, environment, args),
        Command::User(args) => run_user_command(cli, environment, args, input).await,
        Command::Workspace(args) => run_workspace_command(cli, environment, args, input).await,
        Command::Squad(args) => run_squad_command(cli, environment, args).await,
        Command::Label(args) => run_label_command(cli, environment, args).await,
        Command::Project(args) => run_project_command(cli, environment, args).await,
        Command::Property(args) => run_property_command(cli, environment, args).await,
        Command::Chat(args) => run_chat_command(cli, environment, args).await,
        Command::Attachment(args) => run_attachment_command(cli, environment, args).await,
        Command::Repo(RepoArgs {
            command: RepoCommand::List { output },
        }) => run_repo_list(cli, environment, *output).await,
        Command::Repo(RepoArgs {
            command: RepoCommand::Add(args),
        }) => run_repo_add(cli, environment, args).await,
        Command::Repo(RepoArgs {
            command: RepoCommand::Remove(args),
        }) => run_repo_remove(cli, environment, args).await,
        Command::Repo(RepoArgs {
            command: RepoCommand::Checkout { url, checkout_ref },
        }) => run_repo_checkout(environment, url, checkout_ref.as_deref()).await,
        Command::Runtime(RuntimeArgs {
            command: RuntimeCommand::List { output },
        }) => run_runtime_list(cli, environment, *output).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Usage {
                    runtime_id,
                    output,
                    days,
                },
        }) => run_runtime_usage(cli, environment, runtime_id, *output, *days).await,
        Command::Runtime(RuntimeArgs {
            command: RuntimeCommand::Activity { runtime_id, output },
        }) => run_runtime_activity(cli, environment, runtime_id, *output).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Rename {
                    runtime_id,
                    name,
                    machine,
                    output,
                },
        }) => run_runtime_rename(cli, environment, runtime_id, name, *machine, *output).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Delete {
                    runtime_id,
                    cascade,
                    output,
                },
        }) => run_runtime_delete(cli, environment, runtime_id, *cascade, *output).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Update {
                    runtime_id,
                    target_version,
                    output,
                    wait,
                },
        }) => {
            run_runtime_update(
                cli,
                environment,
                runtime_id,
                target_version.as_deref(),
                *output,
                *wait,
            )
            .await
        }
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::List { output },
                }),
        }) => run_runtime_profile_list(cli, environment, *output).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::Create(args),
                }),
        }) => run_runtime_profile_create(cli, environment, args).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::Update(args),
                }),
        }) => run_runtime_profile_update(cli, environment, args).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::Delete { profile_id },
                }),
        }) => run_runtime_profile_delete(cli, environment, profile_id).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::SetPath { profile_id, path },
                }),
        }) => run_runtime_profile_set_path(cli, environment, profile_id, path.as_deref()),
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::UnsetPath { profile_id },
                }),
        }) => run_runtime_profile_unset_path(cli, environment, profile_id),
        Command::Daemon(DaemonArgs {
            command: DaemonCommand::Start(args),
        }) => run_daemon_start(cli, environment, args).await,
        Command::Daemon(DaemonArgs {
            command: DaemonCommand::Status(args),
        }) => run_daemon_status(cli, environment, args).await,
        Command::Daemon(DaemonArgs {
            command: DaemonCommand::Logs(args),
        }) => run_daemon_logs(cli, environment, args).await,
        Command::Daemon(DaemonArgs {
            command: DaemonCommand::Restart(args),
        }) => run_daemon_restart(cli, environment, args).await,
        Command::Daemon(DaemonArgs {
            command: DaemonCommand::Stop,
        }) => run_daemon_stop(cli, environment).await,
        Command::Daemon(DaemonArgs {
            command: DaemonCommand::ProbeRuntimes,
        }) => run_daemon_probe_runtimes(cli, environment),
        Command::Daemon(DaemonArgs {
            command: DaemonCommand::DiskUsage(args),
        }) => run_daemon_disk_usage(cli, environment, args).await,
        Command::Setup(args) => run_setup(cli, environment, args, input).await,
        Command::Update(args) => run_update(cli, environment, args).await,
        Command::Version { output } => run_version(*output),
    }
}
