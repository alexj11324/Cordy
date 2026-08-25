//! Command-facing daemon diagnostics.
//!
//! Runtime probing and disk-usage reporting stay at the CLI boundary while
//! filesystem/API primitives remain owned by daemon and disk-usage modules.

use anyhow::{Context, Result};

use super::config::Environment;
use super::{Cli, RunOutput};

pub(crate) fn run_daemon_probe_runtimes(cli: &Cli, environment: &Environment) -> Result<RunOutput> {
    super::require_human_local_command(environment, "daemon probe-runtimes")?;
    let profile = environment
        .load_config(&cli.profile)
        .context("load daemon probe profile")?;
    let options = profile.daemon_runtime_probe_options(&cli.profile);
    let report =
        cordy_daemon::runtime_probe::probe_runtimes(options).context("probe local runtimes")?;
    Ok(RunOutput {
        stdout: serde_json::to_string(&report)? + "\n",
        stderr: String::new(),
    })
}
