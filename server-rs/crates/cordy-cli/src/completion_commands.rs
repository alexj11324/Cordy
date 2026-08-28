use anyhow::{Context, Result};
use clap::CommandFactory;

use super::{Cli, RunOutput};

pub(super) fn run_completion(shell: clap_complete::Shell) -> Result<RunOutput> {
    let mut command = Cli::command();
    let mut output = Vec::new();
    clap_complete::generate(shell, &mut command, "cordy", &mut output);
    Ok(RunOutput {
        stdout: String::from_utf8(output).context("render shell completion as UTF-8")?,
        stderr: String::new(),
    })
}
