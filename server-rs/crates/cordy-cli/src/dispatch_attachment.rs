//! Attachment command dispatch.
//!
//! Download and upload routing stays together while preserving output
//! directory and optional task-scope handling.

use super::*;

pub(super) async fn run_attachment_command(
    cli: &Cli,
    environment: &Environment,
    args: &AttachmentArgs,
) -> Result<RunOutput> {
    match args {
        AttachmentArgs {
            command:
                AttachmentCommand::Download {
                    attachment_id,
                    output_dir,
                },
        } => run_attachment_download(cli, environment, attachment_id, output_dir).await,
        AttachmentArgs {
            command: AttachmentCommand::Upload { path, task },
        } => run_attachment_upload(cli, environment, path, task.as_deref()).await,
    }
}
