use clap::{Args, Subcommand};
use std::path::PathBuf;

use super::OutputFormat;

#[derive(Debug, Args)]
pub(super) struct AttachmentArgs {
    #[command(subcommand)]
    pub(super) command: AttachmentCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum AttachmentCommand {
    #[command(about = "Download an attachment to a local file")]
    Download {
        #[arg(value_name = "ATTACHMENT-ID")]
        attachment_id: String,
        #[arg(
            short = 'o',
            long,
            default_value = ".",
            help = "Directory to save the downloaded file"
        )]
        output_dir: PathBuf,
    },
    #[command(about = "Upload a file to attach to your chat reply")]
    Upload {
        #[arg(value_name = "PATH")]
        path: PathBuf,
        #[arg(long, help = "Chat task id to attach to (defaults to CORDY_TASK_ID)")]
        task: Option<String>,
    },
}

#[derive(Debug, Args)]
pub(super) struct ChatArgs {
    #[command(subcommand)]
    pub(super) command: ChatCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum ChatCommand {
    #[command(about = "Overview of the channel this conversation is in (messages + thread list)")]
    History(ChatReadArgs),
    #[command(about = "Read one thread's messages (the current thread, or a specific id)")]
    Thread(ChatThreadArgs),
}

#[derive(Debug, Args)]
pub(super) struct ChatReadArgs {
    #[arg(
        long,
        default_value_t = 0,
        help = "Maximum number of messages to return (the server clamps the range)"
    )]
    pub(super) limit: i64,
    #[arg(
        long,
        help = "Opaque cursor (a next_cursor from a prior page) to read older messages"
    )]
    pub(super) before: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct ChatThreadArgs {
    #[arg(value_name = "ID")]
    pub(super) id: Option<String>,
    #[command(flatten)]
    pub(super) read: ChatReadArgs,
}
