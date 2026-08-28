//! CLI update orchestration.
//!
//! Download, checksum, installation, and rollback remain in the daemon
//! update facade; this module owns only command policy and presentation.

use anyhow::Result;
use std::fmt::Write;
use std::io::Write as IoWrite;
use std::time::Duration;

use super::config::Environment;
use super::{
    require_human_local_command, Cli, RunOutput, UpdateArgs, BUILD_COMMIT, BUILD_DATE,
    CLIENT_VERSION,
};

pub(crate) async fn run_update(
    _cli: &Cli,
    environment: &Environment,
    args: &UpdateArgs,
) -> Result<RunOutput> {
    require_human_local_command(environment, "update")?;
    let download_timeout = resolve_update_download_timeout(args);
    validate_update_timeout(download_timeout)?;

    write_update_progress(
        std::io::stderr(),
        &format!(
            "Current version: {CLIENT_VERSION} (commit: {BUILD_COMMIT}, built: {BUILD_DATE})\nChecking installation and latest release...\n"
        ),
    )?;

    let executor = cordy_daemon::update_executor::UpdateExecutor::detect()
        .await
        .map_err(|error| sanitize_update_error(error.into()))?;
    write_update_progress(std::io::stderr(), "Applying update...\n")?;
    let outcome = executor
        .update_request(
            cordy_daemon::update_executor::UpdateRequest::latest()
                .with_current_version(CLIENT_VERSION)
                .with_download_timeout(download_timeout),
        )
        .await
        .map_err(sanitize_update_error)?;

    Ok(render_update_outcome(outcome))
}

pub(crate) fn write_update_progress(mut writer: impl IoWrite, message: &str) -> Result<()> {
    writer.write_all(message.as_bytes())?;
    writer.flush()?;
    Ok(())
}

pub(crate) fn resolve_update_download_timeout(args: &UpdateArgs) -> Duration {
    args.download_timeout
        .unwrap_or(cordy_daemon::update_executor::DEFAULT_UPDATE_DOWNLOAD_TIMEOUT)
}

pub(crate) fn validate_update_timeout(timeout: Duration) -> Result<()> {
    anyhow::ensure!(
        !timeout.is_zero(),
        "download timeout must be greater than zero"
    );
    Ok(())
}

fn sanitize_update_error(error: anyhow::Error) -> anyhow::Error {
    let code = error
        .downcast_ref::<cordy_daemon::update_executor::UpdateExecutorError>()
        .map(|error| error.kind.code())
        .unwrap_or("unknown");
    anyhow::anyhow!("update failed ({code})")
}

pub(crate) fn render_update_outcome(
    outcome: cordy_daemon::update_executor::UpdateOutcome,
) -> RunOutput {
    let mut stderr = String::new();
    if outcome.latest_query_failed {
        let _ = writeln!(
            stderr,
            "Warning: could not check latest version; continuing."
        );
    }
    if outcome.already_current {
        let _ = writeln!(stderr, "Already up to date.");
        return RunOutput {
            stdout: String::new(),
            stderr,
        };
    }

    let latest = outcome
        .resolved_version
        .as_deref()
        .map(str::trim)
        .filter(|version| cordy_daemon::auto_update::is_release_version(version));
    if let Some(version) = latest {
        let _ = writeln!(stderr, "Latest version:  {version}\n");
    }

    match outcome.method {
        cordy_daemon::update_executor::UpdateInstallMethod::Homebrew => {
            let _ = writeln!(stderr, "Updating via Homebrew...");
        }
        cordy_daemon::update_executor::UpdateInstallMethod::Direct => {
            let label = latest.unwrap_or("latest release");
            let _ = writeln!(stderr, "Downloading {label} from GitHub Releases...");
        }
    }
    if !outcome.message.trim().is_empty() {
        let _ = writeln!(stderr, "{}", outcome.message.trim());
    }
    let _ = writeln!(stderr, "Update complete.");
    RunOutput {
        stdout: String::new(),
        stderr,
    }
}
