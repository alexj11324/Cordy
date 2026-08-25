//! Update command dispatch.
//!
//! Update remains a focused route so target selection and updater error
//! handling stay explicit at the command boundary.

use super::*;

pub(super) async fn run_update_command(
    cli: &Cli,
    environment: &Environment,
    args: &UpdateArgs,
) -> Result<RunOutput> {
    run_update(cli, environment, args).await
}
