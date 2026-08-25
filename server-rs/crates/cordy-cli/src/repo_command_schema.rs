use clap::{Args, Subcommand};

use super::OutputFormat;

#[derive(Debug, Args)]
pub(super) struct RepoArgs {
    #[command(subcommand)]
    pub(super) command: RepoCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum RepoCommand {
    #[command(about = "List workspace repositories")]
    List {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Add repositories to the workspace registry")]
    Add(RepoMutationArgs),
    #[command(
        alias = "rm",
        about = "Remove repositories from the workspace registry"
    )]
    Remove(RepoRemoveArgs),
    #[command(about = "Check out a repository into the working directory")]
    Checkout {
        #[arg(value_name = "URL")]
        url: String,
        #[arg(
            long = "ref",
            help = "branch, tag, or commit to check out instead of the remote default branch"
        )]
        checkout_ref: Option<String>,
    },
}

#[derive(Debug, Args)]
pub(super) struct RepoMutationArgs {
    #[arg(value_name = "URL")]
    pub(super) urls: Vec<String>,
    #[arg(long = "url", action = clap::ArgAction::Append, help = "Repository URL (may be repeated)")]
    pub(super) flag_urls: Vec<String>,
    #[arg(long, help = "Optional description; only valid when adding one URL")]
    pub(super) description: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct RepoRemoveArgs {
    #[arg(value_name = "URL")]
    pub(super) urls: Vec<String>,
    #[arg(long = "url", action = clap::ArgAction::Append, help = "Repository URL to remove (may be repeated)")]
    pub(super) flag_urls: Vec<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}
