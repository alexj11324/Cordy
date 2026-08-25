//! Configuration command dispatch.
//!
//! Configuration display and mutation remain centralized here while the root
//! command router stays focused on top-level domain selection.

use super::*;

pub(super) fn run_config_command(
    cli: &Cli,
    environment: &Environment,
    args: &ConfigArgs,
) -> Result<RunOutput> {
    match args {
        ConfigArgs { command: None } => run_config_show(cli, environment, OutputFormat::Table),
        ConfigArgs {
            command: Some(ConfigCommand::Show { output }),
        } => run_config_show(cli, environment, *output),
        ConfigArgs {
            command: Some(ConfigCommand::Set { key, value }),
        } => run_config_set(cli, environment, key, value),
    }
}
