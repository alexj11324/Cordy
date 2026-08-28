use std::fmt::Write;

use super::format_table;

pub(super) fn append_disk_usage_warning(stderr: &mut String) {
    if stderr.is_empty() {
        stderr.push_str("warning: could not resolve issue statuses; STATUS column may be blank\n");
    }
}

pub(super) fn format_disk_usage_report_table(
    report: &patchbay_daemon::diskusage::DiskUsageReport,
    by_workspace: bool,
) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "Workspaces root: {}", report.workspaces_root);
    if by_workspace && report.total_workspace_count == 0 {
        output.push_str("(no workspaces)\n");
        append_disk_usage_repo_cache_line(&mut output, report);
        return output;
    }
    if !by_workspace && report.total_task_count == 0 {
        output.push_str("(no task directories)\n");
        append_disk_usage_repo_cache_line(&mut output, report);
        return output;
    }
    if by_workspace {
        if report.total_workspace_count == 0 {
            output.push_str("(no workspaces)\n");
        }
        let mut rows = vec![vec![
            "WORKSPACE".into(),
            "TASKS".into(),
            "SIZE".into(),
            "ARTIFACTS".into(),
            "ARTIFACT %".into(),
            "OLDEST".into(),
        ]];
        rows.extend(report.workspaces.iter().map(|workspace| {
            vec![
                workspace.workspace_short.clone(),
                workspace.task_count.to_string(),
                format_disk_bytes(workspace.size_bytes),
                format_disk_bytes(workspace.artifact_size_bytes),
                format_disk_ratio(workspace.artifact_ratio),
                format_disk_age(workspace.oldest_age_seconds),
            ]
        }));
        output.push_str(&format_table(&rows));
        if report.workspaces.len() < usize::try_from(report.total_workspace_count).unwrap_or(0) {
            let _ = writeln!(
                output,
                "Showing top {} of {} workspace(s).",
                report.workspaces.len(),
                report.total_workspace_count
            );
        }
        let _ = writeln!(
            output,
            "Total: {} across {} workspace(s); {} reclaimable as artifacts ({}).",
            format_disk_bytes(report.total_size_bytes),
            report.total_workspace_count,
            format_disk_bytes(report.total_artifact_size_bytes),
            format_disk_ratio(report.total_artifact_ratio),
        );
    } else {
        if report.total_task_count == 0 {
            output.push_str("(no task directories)\n");
        }
        let mut rows = vec![vec![
            "PATH".into(),
            "KIND".into(),
            "STATUS".into(),
            "AGE".into(),
            "SIZE".into(),
            "ARTIFACTS".into(),
        ]];
        rows.extend(report.tasks.iter().map(|task| {
            vec![
                format!("{}/{}", task.workspace_short, task.task_short),
                task.kind.clone(),
                if task.parent_status.is_empty() {
                    "-".into()
                } else {
                    task.parent_status.clone()
                },
                format_disk_age(task.age_seconds),
                format_disk_bytes(task.size_bytes),
                format_disk_bytes(task.artifact_size_bytes),
            ]
        }));
        output.push_str(&format_table(&rows));
        if report.tasks.len() < usize::try_from(report.total_task_count).unwrap_or(0) {
            let _ = writeln!(
                output,
                "Showing top {} of {} task(s).",
                report.tasks.len(),
                report.total_task_count
            );
        }
        let _ = writeln!(
            output,
            "Total: {} across {} task(s); {} reclaimable as artifacts ({}).",
            format_disk_bytes(report.total_size_bytes),
            report.total_task_count,
            format_disk_bytes(report.total_artifact_size_bytes),
            format_disk_ratio(report.total_artifact_ratio),
        );
    }
    append_disk_usage_repo_cache_line(&mut output, report);
    output
}

fn append_disk_usage_repo_cache_line(
    output: &mut String,
    report: &patchbay_daemon::diskusage::DiskUsageReport,
) {
    if report.repo_cache_count == 0 && report.repo_cache_size_bytes == 0 {
        return;
    }
    let _ = writeln!(
        output,
        "Repo cache (.repos): {} across {} repo(s), not included above.",
        format_disk_bytes(report.repo_cache_size_bytes),
        report.repo_cache_count,
    );
}

pub(super) fn format_disk_usage_aggregate_table(
    aggregate: &patchbay_daemon::diskusage::AggregateDiskUsageReport,
    by_workspace: bool,
) -> String {
    let mut output = String::new();
    let _ = writeln!(
        output,
        "Scanned {} workspace root(s).",
        aggregate.roots.len()
    );
    for root in &aggregate.roots {
        let _ = writeln!(
            output,
            "\n[{}]",
            if root.profile.is_empty() {
                "default"
            } else {
                &root.profile
            }
        );
        output.push_str(&format_disk_usage_report_table(&root.report, by_workspace));
    }
    let _ = writeln!(
        output,
        "\nGrand total: {} across {} task(s) in {} root(s); {} reclaimable as artifacts ({}).",
        format_disk_bytes(aggregate.total_size_bytes),
        aggregate.total_task_count,
        aggregate.roots.len(),
        format_disk_bytes(aggregate.total_artifact_size_bytes),
        format_disk_ratio(aggregate.total_artifact_ratio),
    );
    if aggregate.total_repo_cache_count > 0 || aggregate.total_repo_cache_size_bytes > 0 {
        let _ = writeln!(
            output,
            "Repo cache (.repos): {} across {} repo(s) in all roots, not included above.",
            format_disk_bytes(aggregate.total_repo_cache_size_bytes),
            aggregate.total_repo_cache_count,
        );
    }
    output
}

pub(super) fn format_disk_ratio(value: f64) -> String {
    if !value.is_finite() || value < 0.0 {
        return "0.0%".into();
    }
    format!("{:.1}%", value * 100.0)
}

fn format_disk_bytes(bytes: i64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut divisor = 1024_i64;
    let mut exponent = 0_usize;
    let mut value = bytes / 1024;
    while value >= 1024 && exponent < 5 {
        divisor *= 1024;
        value /= 1024;
        exponent += 1;
    }
    let prefix = ['K', 'M', 'G', 'T', 'P', 'E'][exponent];
    format!("{:.1} {prefix}iB", bytes as f64 / divisor as f64)
}

fn format_disk_age(seconds: i64) -> String {
    if seconds <= 0 {
        return "0s".into();
    }
    if seconds >= 86_400 {
        return format!("{}d {}h", seconds / 86_400, (seconds % 86_400) / 3_600);
    }
    if seconds >= 3_600 {
        return format!("{}h {}m", seconds / 3_600, (seconds % 3_600) / 60);
    }
    if seconds >= 60 {
        return format!("{}m {}s", seconds / 60, seconds % 60);
    }
    format!("{seconds}s")
}
