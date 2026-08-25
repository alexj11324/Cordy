//! Chat command dispatch.
//!
//! History and thread reads share a focused route module while preserving
//! endpoint, identifier, and read-mode semantics.

use super::*;

pub(super) async fn run_chat_command(
    cli: &Cli,
    environment: &Environment,
    args: &ChatArgs,
) -> Result<RunOutput> {
    match args {
        ChatArgs {
            command: ChatCommand::History(args),
        } => run_chat_read(cli, environment, "/api/chat/history", None, args, true).await,
        ChatArgs {
            command: ChatCommand::Thread(args),
        } => {
            run_chat_read(
                cli,
                environment,
                "/api/chat/thread",
                args.id.as_deref(),
                &args.read,
                false,
            )
            .await
        }
    }
}
