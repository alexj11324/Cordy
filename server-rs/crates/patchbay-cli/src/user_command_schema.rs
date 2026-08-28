use clap::{Args, Subcommand};
use std::path::PathBuf;

use super::*;

#[derive(Debug, Args)]
pub(super) struct UserArgs {
    #[command(subcommand)]
    pub(super) command: UserCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum UserCommand {
    #[command(about = "Get or update your personal profile")]
    Profile(ProfileArgs),
}

#[derive(Debug, Args)]
pub(super) struct ProfileArgs {
    #[command(subcommand)]
    pub(super) command: ProfileCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum ProfileCommand {
    #[command(about = "Show your current user profile")]
    Get {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(
        about = "Update your user profile (currently: profile description)",
        long_about = "Set the personal profile description that gets injected into agent briefs as `## Requesting User`. Pass an empty value to clear it.\n\nPick the input mode that preserves your content:\n  --description \"...\"          inline (decodes \\n / \\t escapes)\n  --description-stdin           pipe a HEREDOC (preserves verbatim)\n  --description-file <path>     read a UTF-8 file (Windows-safe)"
    )]
    Update(UpdateProfileArgs),
}

#[derive(Debug, Args)]
pub(super) struct UpdateProfileArgs {
    #[arg(
        long,
        help = "New profile description (decodes \\n, \\r, \\t, \\\\; use --description-stdin to preserve literal backslashes)"
    )]
    pub(super) description: Option<String>,
    #[arg(
        long,
        help = "Read description from stdin (preserves multi-line content verbatim)"
    )]
    pub(super) description_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read description from a UTF-8 file inside the current working directory"
    )]
    pub(super) description_file: Option<PathBuf>,
    #[arg(
        long,
        help = "Allow --description-file to read outside the current working directory"
    )]
    pub(super) allow_external_file: bool,
    #[arg(
        long,
        help = "Clear the profile description (equivalent to --description \"\")"
    )]
    pub(super) clear: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}
