//! Repository command dispatch.
//!
//! Repository listing, mutation, and checkout routing stays in one module
//! while preserving optional ref handling and environment-only checkout.

use super::*;

pub(super) async fn run_repo_command(
    cli: &Cli,
    environment: &Environment,
    args: &RepoArgs,
) -> Result<RunOutput> {
    match args {
        RepoArgs {
            command: RepoCommand::List { output },
        } => run_repo_list(cli, environment, *output).await,
        RepoArgs {
            command: RepoCommand::Add(args),
        } => run_repo_add(cli, environment, args).await,
        RepoArgs {
            command: RepoCommand::Remove(args),
        } => run_repo_remove(cli, environment, args).await,
        RepoArgs {
            command: RepoCommand::Checkout { url, checkout_ref },
        } => run_repo_checkout(environment, url, checkout_ref.as_deref()).await,
    }
}
