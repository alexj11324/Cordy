use clap::{Parser, Subcommand};

use super::*;

#[derive(Debug, Parser)]
#[command(
    name = "patchbay",
    version = CLIENT_VERSION,
    long_version = ROOT_LONG_VERSION,
    about = "Patchbay CLI — local agent runtime and management tool",
    long_about = "Work seamlessly with Patchbay from the command line."
)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        help = "Patchbay server URL (env: PATCHBAY_SERVER_URL)"
    )]
    pub(super) server_url: Option<String>,
    #[arg(
        long,
        global = true,
        help = "Workspace ID (env: PATCHBAY_WORKSPACE_ID)"
    )]
    pub(super) workspace_id: Option<String>,
    #[arg(
        long,
        global = true,
        default_value = "",
        help = "Configuration profile name (e.g. dev)"
    )]
    pub(super) profile: String,
    #[arg(
        long,
        global = true,
        help = "Print full error details on failure (env: PATCHBAY_DEBUG)"
    )]
    pub(super) debug: bool,
    #[command(subcommand)]
    pub(super) command: Command,
}

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    #[command(about = "Work with agents")]
    Agent(AgentArgs),
    #[command(about = "Work with skills")]
    Skill(SkillArgs),
    #[command(about = "Work with issues")]
    Issue(IssueArgs),
    #[command(about = "Authenticate patchbay with Patchbay")]
    Auth(AuthArgs),
    #[command(about = "Authenticate and set up workspaces")]
    Login(LoginArgs),
    #[command(about = "Manage configuration for patchbay")]
    Config(ConfigArgs),
    #[command(about = "Work with your user account")]
    User(UserArgs),
    #[command(about = "Work with workspaces")]
    Workspace(WorkspaceArgs),
    #[command(about = "Work with teams")]
    Team(TeamArgs),
    #[command(about = "Work with issue labels")]
    Label(LabelArgs),
    #[command(about = "Work with projects")]
    Project(ProjectArgs),
    #[command(about = "Manage workspace custom issue properties")]
    Property(PropertyArgs),
    #[command(about = "Work with the current chat conversation")]
    Chat(ChatArgs),
    #[command(about = "Work with attachments")]
    Attachment(AttachmentArgs),
    #[command(about = "Work with repositories")]
    Repo(RepoArgs),
    #[command(about = "Work with agent runtimes")]
    Runtime(RuntimeArgs),
    #[command(about = "Run the local Patchbay daemon")]
    Daemon(DaemonArgs),
    #[command(about = "Configure the Patchbay server and authenticate")]
    Setup(SetupArgs),
    #[command(about = "Manage autopilots (scheduled/triggered agent automations)")]
    Autopilot(AutopilotArgs),
    #[command(about = "Update patchbay to the latest version")]
    Update(UpdateArgs),
    // Cobra exposes this contract while hiding it from normal help output.
    // Keep it callable for users who regenerate their shell setup after the
    // Rust cutover without adding noise to the primary command list.
    #[command(hide = true)]
    Completion {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    #[command(about = "Print version information")]
    Version {
        #[arg(long, value_enum, default_value_t = VersionOutput::Text)]
        output: VersionOutput,
    },
}
