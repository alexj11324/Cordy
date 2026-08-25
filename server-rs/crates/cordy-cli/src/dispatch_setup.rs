//! Setup command dispatch.
//!
//! Setup remains a focused top-level route so its profile input and shared
//! stdin forwarding are explicit at the dispatcher boundary.

use std::io::Read;

use super::*;

pub(super) async fn run_setup_command<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &SetupArgs,
    input: &mut R,
) -> Result<RunOutput> {
    run_setup(cli, environment, args, input).await
}
