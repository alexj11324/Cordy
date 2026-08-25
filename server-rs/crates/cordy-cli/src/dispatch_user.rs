//! User/profile command dispatch.
//!
//! Profile reads and mutations share this focused boundary while preserving
//! the existing profile handlers and request-body forwarding.

use std::io::Read;

use super::*;

pub(super) async fn run_user_command<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &UserArgs,
    input: &mut R,
) -> Result<RunOutput> {
    match args {
        UserArgs {
            command:
                UserCommand::Profile(ProfileArgs {
                    command: ProfileCommand::Get { output },
                }),
        } => run_user_profile_get(cli, environment, *output).await,
        UserArgs {
            command:
                UserCommand::Profile(ProfileArgs {
                    command: ProfileCommand::Update(args),
                }),
        } => run_user_profile_update(cli, environment, args, input).await,
    }
}
