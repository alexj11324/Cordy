//! Daemon disk-usage command orchestration.
//!
//! Argument policy and rendering remain in the disk-usage helpers; this module
//! coordinates one-profile and all-profile scans with optional parent status
//! enrichment.

use anyhow::Result;

use super::{Cli, DaemonDiskUsageArgs, OutputFormat, RunOutput};

pub(crate) async fn run_daemon_disk_usage(
    cli: &Cli,
    environment: &super::config::Environment,
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
