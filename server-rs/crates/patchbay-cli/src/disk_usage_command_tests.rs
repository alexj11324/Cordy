use super::*;
use clap::Parser;

#[test]
fn daemon_disk_usage_parses_all_typed_flags() {
    let cli = Cli::try_parse_from([
        "patchbay",
        "--profile",
        "staging",
        "daemon",
        "disk-usage",
        "--by-workspace",
        "--top",
        "7",
        "--output",
        "json",
        "--workspaces-root",
        "/var/lib/patchbay/workspaces",
        "--all-profiles",
    ])
    .expect("disk-usage args");
    let Command::Daemon(DaemonArgs {
        command: DaemonCommand::DiskUsage(args),
    }) = cli.command
    else {
        panic!("expected daemon disk-usage");
    };
    assert!(args.by_workspace);
    assert!(!args.by_task);
    assert_eq!(args.top, 7);
    assert_eq!(args.output, OutputFormat::Json);
    assert_eq!(
        args.workspaces_root.as_deref(),
        Some("/var/lib/patchbay/workspaces")
    );
    assert!(args.all_profiles);
}

#[test]
fn daemon_disk_usage_validation_rejects_unsafe_or_incomplete_modes() {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    let cli = Cli::try_parse_from(["patchbay", "daemon", "disk-usage"]).expect("CLI");
    let both = DaemonDiskUsageArgs {
        by_workspace: true,
        by_task: true,
        top: 0,
        output: OutputFormat::Json,
        workspaces_root: None,
        all_profiles: false,
    };
    assert!(validate_disk_usage_args(&cli, &environment, &both, false)
        .expect_err("conflicting views")
        .to_string()
        .contains("mutually exclusive"));

    let negative = DaemonDiskUsageArgs {
        by_workspace: false,
        by_task: false,
        top: -1,
        output: OutputFormat::Json,
        workspaces_root: None,
        all_profiles: false,
    };
    assert!(
        validate_disk_usage_args(&cli, &environment, &negative, false)
            .expect_err("negative top")
            .to_string()
            .contains("non-negative")
    );

    let all_profiles_with_override = DaemonDiskUsageArgs {
        by_workspace: false,
        by_task: false,
        top: 0,
        output: OutputFormat::Table,
        workspaces_root: Some("/tmp/workspaces".into()),
        all_profiles: true,
    };
    assert!(
        validate_disk_usage_args(&cli, &environment, &all_profiles_with_override, false)
            .expect_err("all-profiles root override")
            .to_string()
            .contains("mutually exclusive")
    );

    environment.set("PATCHBAY_TASK_WORKSPACES_ROOT", "/srv/patchbay/workspaces");
    assert!(validate_disk_usage_args(
        &cli,
        &environment,
        &DaemonDiskUsageArgs {
            by_workspace: false,
            by_task: false,
            top: 0,
            output: OutputFormat::Json,
            workspaces_root: None,
            all_profiles: true,
        },
        true
    )
    .expect_err("task cannot enumerate owner profiles")
    .to_string()
    .contains("--all-profiles"));
}

#[test]
fn daemon_disk_usage_task_scope_requires_absolute_injected_root() {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_AGENT_ID", "agent-1");
    let cli = Cli::try_parse_from(["patchbay", "daemon", "disk-usage"]).expect("CLI");
    let args = DaemonDiskUsageArgs {
        by_workspace: false,
        by_task: false,
        top: 0,
        output: OutputFormat::Json,
        workspaces_root: None,
        all_profiles: false,
    };
    let missing = validate_disk_usage_args(&cli, &environment, &args, true)
        .expect_err("task must carry an injected root");
    assert!(missing.to_string().contains("PATCHBAY_TASK_WORKSPACES_ROOT"));

    environment.set("PATCHBAY_TASK_WORKSPACES_ROOT", "relative/workspaces");
    let relative = resolve_disk_usage_root(&cli, &environment, &args, true)
        .expect_err("task root must be absolute");
    assert!(relative.to_string().contains("absolute path"));

    environment.set("PATCHBAY_TASK_WORKSPACES_ROOT", "/srv/patchbay/workspaces");
    assert_eq!(
        resolve_disk_usage_root(&cli, &environment, &args, true).expect("absolute root"),
        "/srv/patchbay/workspaces"
    );
}

#[test]
fn daemon_disk_usage_json_preserves_scan_totals_when_limiting_rows() {
    let report = patchbay_daemon::diskusage::DiskUsageReport {
        workspaces_root: "/srv/workspaces".into(),
        tasks: vec![
            patchbay_daemon::diskusage::TaskDiskUsage {
                path: "/srv/workspaces/ws/task-a".into(),
                size_bytes: 20,
                ..Default::default()
            },
            patchbay_daemon::diskusage::TaskDiskUsage {
                path: "/srv/workspaces/ws/task-b".into(),
                size_bytes: 10,
                ..Default::default()
            },
        ],
        total_task_count: 2,
        total_size_bytes: 30,
        ..Default::default()
    };
    let args = DaemonDiskUsageArgs {
        by_workspace: false,
        by_task: true,
        top: 1,
        output: OutputFormat::Json,
        workspaces_root: None,
        all_profiles: false,
    };
    let mut limited = report;
    limit_disk_usage_report(&mut limited, &args);
    let json = serde_json::to_value(&limited).expect("disk-usage JSON");
    assert_eq!(json["workspaces_root"], "/srv/workspaces");
    assert_eq!(json["tasks"].as_array().map(|tasks| tasks.len()), Some(1));
    assert_eq!(json["total_task_count"], 2);
    assert_eq!(json["total_size_bytes"], 30);
}

#[test]
fn daemon_disk_usage_table_formats_task_status_and_iec_sizes() {
    let report = patchbay_daemon::diskusage::DiskUsageReport {
        workspaces_root: "/srv/workspaces".into(),
        tasks: vec![patchbay_daemon::diskusage::TaskDiskUsage {
            workspace_short: "workspace".into(),
            task_short: "task".into(),
            kind: "issue".into(),
            parent_status: "in_progress".into(),
            age_seconds: 3_661,
            size_bytes: 2_048,
            artifact_size_bytes: 1_024,
            ..Default::default()
        }],
        total_task_count: 1,
        total_size_bytes: 2_048,
        total_artifact_size_bytes: 1_024,
        total_artifact_ratio: 0.5,
        ..Default::default()
    };
    let table = format_disk_usage_report_table(&report, false);
    assert!(table.contains("PATH"));
    assert!(table.contains("workspace/task"));
    assert!(table.contains("in_progress"));
    assert!(table.contains("2.0 KiB"));
    assert!(table.contains("1h 1m"));
    assert!(table.contains("50.0%"));
}

#[test]
fn daemon_disk_usage_empty_tables_match_go_diagnostics() {
    let report = patchbay_daemon::diskusage::DiskUsageReport {
        workspaces_root: "/srv/workspaces".into(),
        ..Default::default()
    };
    let task_table = format_disk_usage_report_table(&report, false);
    let workspace_table = format_disk_usage_report_table(&report, true);
    assert!(task_table.contains("(no task directories)"));
    assert!(workspace_table.contains("(no workspaces)"));
    assert!(!task_table.contains("PATH"));
    assert!(!workspace_table.contains("WORKSPACE"));
}

#[test]
fn daemon_disk_usage_aggregate_top_keeps_grand_totals() {
    let mut aggregate = patchbay_daemon::diskusage::AggregateDiskUsageReport {
        roots: vec![patchbay_daemon::diskusage::RootDiskUsage {
            profile: String::new(),
            report: patchbay_daemon::diskusage::DiskUsageReport {
                tasks: vec![
                    patchbay_daemon::diskusage::TaskDiskUsage::default(),
                    patchbay_daemon::diskusage::TaskDiskUsage::default(),
                ],
                total_task_count: 2,
                ..Default::default()
            },
        }],
        total_task_count: 2,
        ..Default::default()
    };
    let args = DaemonDiskUsageArgs {
        by_workspace: false,
        by_task: true,
        top: 1,
        output: OutputFormat::Json,
        workspaces_root: None,
        all_profiles: true,
    };
    limit_disk_usage_aggregate(&mut aggregate, &args);
    assert_eq!(aggregate.roots[0].report.tasks.len(), 1);
    assert_eq!(aggregate.total_task_count, 2);
    assert_eq!(format_disk_ratio(f64::NAN), "0.0%");
}

#[test]
fn daemon_disk_usage_enumerates_default_and_existing_profile_roots() {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let default_root = home.path().join("default-workspaces");
    let profile_root = home.path().join("staging-workspaces");
    fs::create_dir_all(&default_root).expect("default root");
    fs::create_dir_all(&profile_root).expect("profile root");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let default_config = environment.config_path("").expect("default config");
    let profile_config = environment.config_path("staging").expect("profile config");
    fs::create_dir_all(default_config.parent().expect("config directory"))
        .expect("config directory");
    fs::create_dir_all(profile_config.parent().expect("profile directory"))
        .expect("profile directory");
    fs::write(
        default_config,
        serde_json::json!({
            "workspaces_root": default_root.to_string_lossy().to_string()
        })
        .to_string(),
    )
    .expect("default config");
    fs::write(
        profile_config,
        serde_json::json!({
            "workspaces_root": profile_root.to_string_lossy().to_string()
        })
        .to_string(),
    )
    .expect("profile config");

    let roots = enumerate_disk_usage_roots(&environment).expect("profile roots");
    assert_eq!(roots.len(), 2);
    assert_eq!(roots[0].profile, "");
    assert_eq!(roots[0].root, default_root.to_string_lossy().to_string());
    assert_eq!(roots[1].profile, "staging");
    assert_eq!(roots[1].root, profile_root.to_string_lossy().to_string());
}

#[tokio::test]
async fn daemon_disk_usage_status_enrichment_has_one_command_deadline() {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_HTTP_TIMEOUT", "1ms");
    let cancellation = tokio_util::sync::CancellationToken::new();

    let failed = with_disk_usage_status_deadline(
        &environment,
        &cancellation,
        std::future::pending::<bool>(),
    )
    .await;

    assert!(failed);
    assert!(cancellation.is_cancelled());
}
