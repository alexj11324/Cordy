//! Command-facing daemon diagnostics.
//!
//! Runtime probing and disk-usage reporting stay at the CLI boundary while
//! filesystem/API primitives remain owned by daemon and disk-usage modules.

use anyhow::{Context, Result};

use super::config::Environment;
use super::{Cli, DaemonDiskUsageArgs, OutputFormat, RunOutput};

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

/// The CLI owns argument validation and presentation only. Filesystem
/// traversal and parent-status HTTP semantics remain in the existing helpers;
/// this command boundary keeps the daemon diagnostic entry points together.
pub(crate) async fn run_daemon_disk_usage(
    cli: &Cli,
    environment: &Environment,
    args: &DaemonDiskUsageArgs,
) -> Result<RunOutput> {
    let task_context = super::disk_usage_task_context(environment);
    super::validate_disk_usage_args(cli, environment, args, task_context)?;

    let mut stderr = String::new();
    if args.all_profiles {
        let roots = super::enumerate_disk_usage_roots(environment)?;
        let mut aggregate = cordy_daemon::diskusage::scan_disk_usage_roots(
            &roots,
            &cordy_daemon::diskusage::artifact_patterns_from_env(),
        )?;
        if !task_context && super::disk_usage_needs_parent_status(args) {
            for root in &mut aggregate.roots {
                if super::fill_disk_usage_parent_statuses(
                    cli,
                    environment,
                    &root.profile,
                    &mut root.report,
                )
                .await
                {
                    super::append_disk_usage_warning(&mut stderr);
                }
            }
        }
        super::limit_disk_usage_aggregate(&mut aggregate, args);
        let stdout = match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&aggregate)?),
            OutputFormat::Table => {
                super::format_disk_usage_aggregate_table(&aggregate, args.by_workspace)
            }
        };
        return Ok(RunOutput { stdout, stderr });
    }

    let root = super::resolve_disk_usage_root(cli, environment, args, task_context)?;
    let mut report = cordy_daemon::diskusage::scan_disk_usage(
        &root,
        &cordy_daemon::diskusage::artifact_patterns_from_env(),
    )?;
    if !task_context
        && super::disk_usage_needs_parent_status(args)
        && super::fill_disk_usage_parent_statuses(cli, environment, &cli.profile, &mut report).await
    {
        super::append_disk_usage_warning(&mut stderr);
    }
    super::limit_disk_usage_report(&mut report, args);
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&report)?),
        OutputFormat::Table => super::format_disk_usage_report_table(&report, args.by_workspace),
    };
    Ok(RunOutput { stdout, stderr })
}
