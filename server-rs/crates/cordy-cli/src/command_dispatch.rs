use std::io::Read;

use super::dispatch_agent::run_agent_command;
use super::dispatch_attachment::run_attachment_command;
use super::dispatch_auth::{run_auth_command, run_login_command};
use super::dispatch_autopilot::run_autopilot_command;
use super::dispatch_chat::run_chat_command;
use super::dispatch_config::run_config_command;
use super::dispatch_daemon::run_daemon_command;
use super::dispatch_issue::run_issue_command;
use super::dispatch_label::run_label_command;
use super::dispatch_project::run_project_command;
use super::dispatch_property::run_property_command;
use super::dispatch_repo::run_repo_command;
use super::dispatch_runtime::run_runtime_command;
use super::dispatch_setup::run_setup_command;
use super::dispatch_skill::run_skill_command;
use super::dispatch_squad::run_squad_command;
use super::dispatch_update::run_update_command;
use super::dispatch_user::run_user_command;
use super::dispatch_version::run_version_command;
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
        Command::Repo(args) => run_repo_command(cli, environment, args).await,
        Command::Runtime(args) => run_runtime_command(cli, environment, args).await,
        Command::Daemon(args) => run_daemon_command(cli, environment, args).await,
        Command::Setup(args) => run_setup_command(cli, environment, args, input).await,
        Command::Update(args) => run_update_command(cli, environment, args).await,
        Command::Version { output } => run_version_command(*output),
    }
}
