//! Private daemon execution-environment helper entry point.
//!
//! The helper keeps task configuration on inherited pipes and handles the
//! private argv discriminator before normal CLI parsing.

use anyhow::Result;
use std::ffi::OsString;
use std::io::{Read, Write as IoWrite};

pub async fn run_private_helper<I, O>(args: &[OsString], input: I, output: &mut O) -> Result<bool>
where
    I: Read,
    O: IoWrite,
{
    if args.len() != 2 || args[1] != cordy_daemon::execenv::isolation::PREPARATION_HELPER_ARG {
        return Ok(false);
    }
    cordy_daemon::execenv::isolation::run_preparation_helper(input, output).await?;
    Ok(true)
}
