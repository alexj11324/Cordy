use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use super::{lexical_normalize, Cli, DaemonDiskUsageArgs, Environment, OutputFormat};

pub(super) fn disk_usage_task_context(environment: &Environment) -> bool {
    environment.in_daemon_managed_execution_context()
        || environment.in_daemon_task_identity_context()
}

pub(super) fn validate_disk_usage_args(
    cli: &Cli,
    environment: &Environment,
    args: &DaemonDiskUsageArgs,
    task_context: bool,
) -> Result<()> {
    if args.by_workspace && args.by_task {
        bail!("--by-workspace and --by-task are mutually exclusive");
    }
    if args.top < 0 {
        bail!("--top must be a non-negative integer");
    }
    if args.all_profiles
        && args
            .workspaces_root
            .as_deref()
            .is_some_and(|root| !root.trim().is_empty())
    {
        bail!("--all-profiles and --workspaces-root are mutually exclusive");
    }
    if task_context {
        if args.all_profiles {
            bail!("daemon disk-usage --all-profiles is not available inside a daemon-managed task");
        }
        if !cli.profile.trim().is_empty() {
            bail!("daemon disk-usage --profile is not available inside a daemon-managed task");
        }
        if args
            .workspaces_root
            .as_deref()
            .is_some_and(|root| !root.trim().is_empty())
        {
            bail!(
                "daemon disk-usage --workspaces-root is not available inside a daemon-managed task"
            );
        }
        if environment
            .trimmed(cordy_daemon::config::TASK_WORKSPACES_ROOT_ENV)
            .is_none()
        {
            bail!(
                "daemon-managed task requires {}",
                cordy_daemon::config::TASK_WORKSPACES_ROOT_ENV
            );
        }
    }
    Ok(())
}

pub(super) fn resolve_disk_usage_root(
    cli: &Cli,
    environment: &Environment,
    args: &DaemonDiskUsageArgs,
    task_context: bool,
) -> Result<String> {
    if task_context {
        let root = environment
            .trimmed(cordy_daemon::config::TASK_WORKSPACES_ROOT_ENV)
            .context("resolve daemon task workspaces root")?;
        let path = Path::new(root);
        if !path.is_absolute() {
            bail!(
                "{} must be an absolute path",
                cordy_daemon::config::TASK_WORKSPACES_ROOT_ENV
            );
        }
        return Ok(path.to_path_buf().to_string_lossy().into_owned());
    }

    resolve_disk_usage_root_for_profile(environment, &cli.profile, args.workspaces_root.as_deref())
}

pub(super) fn limit_disk_usage_report(
    report: &mut cordy_daemon::diskusage::DiskUsageReport,
    args: &DaemonDiskUsageArgs,
) {
    let Some(limit) = usize::try_from(args.top).ok().filter(|limit| *limit > 0) else {
        return;
    };
    if args.by_workspace {
        report.workspaces.truncate(limit);
    } else {
        report.tasks.truncate(limit);
    }
}

pub(super) fn limit_disk_usage_aggregate(
    aggregate: &mut cordy_daemon::diskusage::AggregateDiskUsageReport,
    args: &DaemonDiskUsageArgs,
) {
    for root in &mut aggregate.roots {
        limit_disk_usage_report(&mut root.report, args);
    }
}

pub(super) fn resolve_disk_usage_root_for_profile(
    environment: &Environment,
    profile: &str,
    flag_root: Option<&str>,
) -> Result<String> {
    let config = environment.load_config(profile).unwrap_or_default();
    let configured_root = flag_root
        .map(str::trim)
        .filter(|root| !root.is_empty())
        .or_else(|| environment.trimmed("CORDY_WORKSPACES_ROOT"))
        .or_else(|| {
            (!config.workspaces_root.trim().is_empty()).then_some(config.workspaces_root.trim())
        })
        .unwrap_or_default();
    cordy_daemon::config::resolve_workspaces_root(profile, configured_root)
        .context("resolve workspaces root")
}

pub(super) fn enumerate_disk_usage_roots(
    environment: &Environment,
) -> Result<Vec<cordy_daemon::diskusage::DiskUsageRoot>> {
    let mut roots = Vec::new();
    let default_root = resolve_disk_usage_root_for_profile(environment, "", None)?;
    roots.push(cordy_daemon::diskusage::DiskUsageRoot {
        profile: String::new(),
        root: default_root,
    });

    let profiles_root = environment
        .config_path("")
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("profiles")));
    let Some(profiles_root) = profiles_root else {
        return Ok(roots);
    };
    let Ok(entries) = fs::read_dir(profiles_root) else {
        return Ok(roots);
    };
    let mut profiles = entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir())
                .and_then(|_| entry.file_name().into_string().ok())
        })
        .collect::<Vec<_>>();
    profiles.sort();
    for profile in profiles {
        let root = match resolve_disk_usage_root_for_profile(environment, &profile, None) {
            Ok(root) => root,
            Err(_) => continue,
        };
        if roots
            .iter()
            .any(|existing| same_disk_usage_path(&existing.root, &root))
        {
            continue;
        }
        if !Path::new(&root).is_dir() {
            continue;
        }
        roots.push(cordy_daemon::diskusage::DiskUsageRoot { profile, root });
    }
    Ok(roots)
}

fn same_disk_usage_path(left: &str, right: &str) -> bool {
    let normalize =
        |path: &str| fs::canonicalize(path).unwrap_or_else(|_| lexical_normalize(Path::new(path)));
    normalize(left) == normalize(right)
}

pub(super) fn disk_usage_needs_parent_status(args: &DaemonDiskUsageArgs) -> bool {
    !args.by_workspace || args.output == OutputFormat::Json
}

pub(super) async fn fill_disk_usage_parent_statuses(
    cli: &Cli,
    environment: &Environment,
    profile: &str,
    report: &mut cordy_daemon::diskusage::DiskUsageReport,
) -> bool {
    if !report
        .tasks
        .iter()
        .any(|task| task.kind == "issue" && !task.parent_id.trim().is_empty())
    {
        return false;
    }
    let config = environment.load_config(profile).unwrap_or_default();
    let token = environment
        .trimmed("CORDY_TOKEN")
        .map(ToOwned::to_owned)
        .or_else(|| (!config.token.trim().is_empty()).then(|| config.token.clone()));
    let Some(token) = token else {
        // Go's fetcher returns nil when the profile is logged out; an offline
        // diagnostic must not warn merely because no credentials are stored.
        return false;
    };
    let raw_server_url = cli
        .server_url
        .as_deref()
        .or_else(|| environment.trimmed("CORDY_SERVER_URL"))
        .or_else(|| (!config.server_url.trim().is_empty()).then_some(config.server_url.as_str()))
        .unwrap_or(cordy_daemon::config::DEFAULT_SERVER_URL);
    let Ok(server_url) = cordy_daemon::config::normalize_server_base_url(raw_server_url) else {
        // An invalid/unconfigured server URL is the same no-fetch case as a
        // missing token. The local report remains useful and warning-free.
        return false;
    };
    let client = Arc::new(cordy_daemon::client::Client::new(server_url));
    client.set_token(&token);
    let resolver = cordy_daemon::diskusage::ClientParentStatusResolver::new(client);
    let cancellation = tokio_util::sync::CancellationToken::new();
    cordy_daemon::diskusage::resolve_parent_statuses(&cancellation, report, &resolver)
        .await
        .is_err()
}
