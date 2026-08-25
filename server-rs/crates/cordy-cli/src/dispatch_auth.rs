//! Authentication and login command dispatch.
//!
//! Auth status/logout and browser/token login share one top-level routing
//! boundary while preserving the existing command handlers and output policy.

use super::*;

pub(super) async fn run_auth_command(
    cli: &Cli,
    environment: &Environment,
    args: &AuthArgs,
) -> Result<RunOutput> {
    match args {
        AuthArgs {
            command: AuthCommand::Status { output },
        } => run_auth_status(cli, environment, *output).await,
        AuthArgs {
            command: AuthCommand::Logout,
        } => run_auth_logout(cli, environment),
    }
}

pub(super) async fn run_login_command(
    cli: &Cli,
    environment: &Environment,
    args: &LoginArgs,
) -> Result<RunOutput> {
    run_login(cli, environment, args).await
}
