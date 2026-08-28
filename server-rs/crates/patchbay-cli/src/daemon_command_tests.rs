use super::*;
use clap::Parser;
use serde_json::Value;
use std::fs;
use std::io::Cursor;
use std::time::Duration;

#[test]
fn daemon_context_never_falls_back_to_owner_credentials() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let config_dir = home.path().join(".patchbay");
    fs::create_dir_all(&config_dir).expect("config dir");
    fs::write(
        config_dir.join("config.json"),
        r#"{"server_url":"https://api.example.com","token":"pby_owner"}"#,
    )
    .expect("config");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_AGENT_ID", "agent-1");
    let cli = Cli::try_parse_from(["patchbay", "user", "profile", "get"]).expect("parse CLI");

    let error = new_api_client(&cli, &environment).expect_err("must fail closed");
    assert!(error.to_string().contains("task-scoped mat_ token"));
}

#[test]
fn daemon_foreground_command_parses_successor_launch_arguments() {
    let cli = Cli::try_parse_from([
        "patchbay",
        "--profile",
        "staging",
        "daemon",
        "start",
        "--foreground",
        "--daemon-id",
        "daemon-1",
        "--poll-interval",
        "3s",
        "--agent-timeout",
        "0s",
        "--health-port",
        "19710",
        "--no-auto-update",
    ])
    .expect("foreground daemon command");
    let Command::Daemon(DaemonArgs {
        command: DaemonCommand::Start(args),
    }) = cli.command
    else {
        panic!("expected daemon start command");
    };
    assert!(args.launch.foreground);
    assert_eq!(args.launch.daemon_id.as_deref(), Some("daemon-1"));
    assert_eq!(args.launch.poll_interval, Some(Duration::from_secs(3)));
    assert_eq!(args.launch.agent_timeout, Some(Duration::ZERO));
    assert_eq!(args.launch.health_port, Some(19710));
    assert!(args.launch.disable_auto_update);
}

#[test]
fn daemon_restart_reuses_start_flags_and_rejects_foreground() {
    let restart = Cli::try_parse_from([
        "patchbay",
        "--profile",
        "staging",
        "daemon",
        "restart",
        "--foreground",
        "--daemon-id",
        "daemon-2",
        "--device-name",
        "laptop",
        "--runtime-name",
        "codex",
        "--workspaces-root",
        "/srv/workspaces",
        "--poll-interval",
        "2s",
        "--heartbeat-interval",
        "7s",
        "--agent-timeout",
        "0s",
        "--codex-semantic-inactivity-timeout",
        "13s",
        "--codex-handshake-timeout",
        "5s",
        "--max-concurrent-tasks",
        "4",
        "--health-port",
        "19711",
        "--no-auto-update",
        "--auto-update-interval",
        "8h",
        "--no-auto-reload",
        "--server-url",
        "https://staging.example",
    ])
    .expect("daemon restart command");
    assert_eq!(
        restart.server_url.as_deref(),
        Some("https://staging.example")
    );
    let Command::Daemon(DaemonArgs {
        command: DaemonCommand::Restart(args),
    }) = restart.command
    else {
        panic!("expected daemon restart command");
    };
    assert!(args.launch.foreground);
    assert_eq!(args.launch.daemon_id.as_deref(), Some("daemon-2"));
    assert_eq!(args.launch.device_name.as_deref(), Some("laptop"));
    assert_eq!(args.launch.runtime_name.as_deref(), Some("codex"));
    assert_eq!(
        args.launch.workspaces_root.as_deref(),
        Some("/srv/workspaces")
    );
    assert_eq!(args.launch.poll_interval, Some(Duration::from_secs(2)));
    assert_eq!(args.launch.heartbeat_interval, Some(Duration::from_secs(7)));
    assert_eq!(args.launch.agent_timeout, Some(Duration::ZERO));
    assert_eq!(
        args.launch.codex_semantic_inactivity_timeout,
        Some(Duration::from_secs(13))
    );
    assert_eq!(
        args.launch.codex_handshake_timeout,
        Some(Duration::from_secs(5))
    );
    assert_eq!(args.launch.max_concurrent_tasks, Some(4));
    assert_eq!(args.launch.health_port, Some(19711));
    assert!(args.launch.disable_auto_update);
    assert_eq!(
        args.launch.auto_update_interval,
        Some(Duration::from_secs(8 * 60 * 60))
    );
    assert!(args.launch.disable_auto_reload);
    let flags = args
        .launch
        .to_launch_flags(Some("https://staging.example".to_string()));
    assert_eq!(flags.server_url.as_deref(), Some("https://staging.example"));
    assert_eq!(flags.daemon_id.as_deref(), Some("daemon-2"));
    assert_eq!(flags.workspaces_root.as_deref(), Some("/srv/workspaces"));
    let error = ensure_restart_is_background(&args.launch)
        .expect_err("restart foreground must fail closed");
    assert!(error
        .to_string()
        .contains("daemon restart does not support"));

    let restart = Cli::try_parse_from([
        "patchbay",
        "--profile",
        "staging",
        "daemon",
        "restart",
        "--daemon-id",
        "daemon-2",
    ])
    .expect("background daemon restart command");
    let Command::Daemon(DaemonArgs {
        command: DaemonCommand::Restart(args),
    }) = restart.command
    else {
        panic!("expected daemon restart command");
    };
    ensure_restart_is_background(&args.launch).expect("background restart is valid");

    let stop = Cli::try_parse_from(["patchbay", "--profile", "staging", "daemon", "stop"])
        .expect("daemon stop command");
    assert!(matches!(
        stop.command,
        Command::Daemon(DaemonArgs {
            command: DaemonCommand::Stop
        })
    ));
}

#[test]
fn daemon_probe_runtimes_is_hidden_but_parses_as_a_local_command() {
    let cli = Cli::try_parse_from([
        "patchbay",
        "--profile",
        "staging",
        "daemon",
        "probe-runtimes",
    ])
    .expect("probe-runtimes command");
    assert_eq!(cli.profile, "staging");
    assert!(matches!(
        cli.command,
        Command::Daemon(DaemonArgs {
            command: DaemonCommand::ProbeRuntimes
        })
    ));
}

#[test]
fn daemon_status_parses_table_and_json_output_modes() {
    let table =
        Cli::try_parse_from(["patchbay", "daemon", "status"]).expect("daemon status table CLI");
    let Command::Daemon(DaemonArgs {
        command: DaemonCommand::Status(args),
    }) = table.command
    else {
        panic!("expected daemon status");
    };
    assert_eq!(args.output, OutputFormat::Table);

    let json = Cli::try_parse_from(["patchbay", "daemon", "status", "--output", "json"])
        .expect("daemon status JSON CLI");
    let Command::Daemon(DaemonArgs {
        command: DaemonCommand::Status(args),
    }) = json.command
    else {
        panic!("expected daemon status");
    };
    assert_eq!(args.output, OutputFormat::Json);
}

#[test]
fn daemon_status_rejects_unknown_profiles_and_lists_nested_known_profiles() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let error = require_known_daemon_profile(&environment, "missing").expect_err("unknown profile");
    assert!(error.to_string().contains("unknown profile \"missing\""));
    assert!(error.to_string().contains("no named profiles exist yet"));

    let nested = home.path().join(".patchbay/profiles/team/dev");
    fs::create_dir_all(&nested).expect("nested profile");
    fs::write(nested.join("config.json"), "{}").expect("profile config");
    assert_eq!(
        known_daemon_profiles(&environment),
        vec!["team/dev".to_string()]
    );
    require_known_daemon_profile(&environment, "team/dev")
        .expect("nested profile should be accepted");
}

#[test]
fn daemon_status_task_port_uses_injected_port_and_rejects_profile_override() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_TASK_ID", "task-1");
    environment.set("PATCHBAY_DAEMON_PORT", "19601");
    let cli = Cli::try_parse_from(["patchbay", "daemon", "status"]).expect("status CLI");
    assert_eq!(
        resolve_daemon_status_port(&cli, &environment).expect("injected port"),
        19601
    );

    let profile_cli = Cli::try_parse_from(["patchbay", "--profile", "staging", "daemon", "status"])
        .expect("profile status CLI");
    let error =
        resolve_daemon_status_port(&profile_cli, &environment).expect_err("task profile override");
    assert!(error
        .to_string()
        .contains("--profile is not available inside a daemon-managed task"));
}

#[test]
fn daemon_status_table_and_collision_json_match_go_diagnostics() {
    let response = patchbay_daemon::health::HealthResponse {
        status: "running".into(),
        pid: 1234,
        uptime: "1h2m3s".into(),
        cli_version: "v9.9.9".into(),
        launched_by: "desktop".into(),
        reload_pending_reason: "waiting for active tasks".into(),
        agents: vec!["codex".into()],
        workspaces: vec![patchbay_daemon::health::HealthWorkspace::default()],
        ..Default::default()
    };
    let table = format_daemon_status_table("Daemon [staging]", &response);
    assert!(table.contains("Daemon [staging]:  running (pid 1234, uptime 1h2m3s)"));
    assert!(table.contains("Managed by:"));
    assert!(table.contains("Patchbay Desktop app (start and stop it from the app)"));
    assert!(table.contains("Restart pending:"));
    assert!(table.contains("Agents:"));
    assert!(table.contains("Workspaces:"));
    assert!(table.contains("  1\n"));

    let conflict = patchbay_daemon::control_client::ProfileMismatch {
        expected: "ab".into(),
        actual: Some("ba".into()),
        port: 19710,
    };
    let json = render_daemon_status(
        "ab",
        OutputFormat::Json,
        patchbay_daemon::control_client::LocalDaemonHealth::Stopped,
        Some(conflict),
    )
    .expect("collision JSON");
    let document: Value = serde_json::from_str(&json.stdout).expect("status JSON");
    assert_eq!(document["status"], "stopped");
    assert_eq!(document["port_conflict"]["port"], 19710);
    assert_eq!(document["port_conflict"]["profile"], "ba");
}

#[test]
fn daemon_logs_parses_follow_and_bounded_line_flags() {
    let cli = Cli::try_parse_from([
        "patchbay",
        "--profile",
        "staging",
        "daemon",
        "logs",
        "--follow",
        "--lines",
        "7",
    ])
    .expect("daemon logs CLI");
    let Command::Daemon(DaemonArgs {
        command: DaemonCommand::Logs(args),
    }) = cli.command
    else {
        panic!("expected daemon logs");
    };
    assert!(args.follow);
    assert_eq!(args.lines, 7);
    assert!(Cli::try_parse_from(["patchbay", "daemon", "logs", "--lines", "-1"]).is_err());
    assert!(Cli::try_parse_from(["patchbay", "daemon", "logs", "--lines", "100001"]).is_err());
}

#[test]
fn daemon_logs_tail_matches_recent_lines_with_or_without_final_newline() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let path = resolve_daemon_log_path(&environment, "").expect("default log path");
    fs::create_dir_all(path.parent().expect("log parent")).expect("log parent");

    fs::write(&path, b"one\ntwo\nthree\n").expect("log with newline");
    assert_eq!(
        read_daemon_log_tail(&path, 2).expect("tail"),
        b"two\nthree\n"
    );
    assert_eq!(read_daemon_log_tail(&path, 0).expect("empty tail"), b"");

    fs::write(&path, b"one\ntwo\nthree").expect("log without newline");
    assert_eq!(read_daemon_log_tail(&path, 2).expect("tail"), b"two\nthree");
}

#[tokio::test]
async fn daemon_logs_is_rejected_inside_a_daemon_task() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_TASK_ID", "task-1");
    let cli = Cli::try_parse_from(["patchbay", "daemon", "logs"]).expect("daemon logs CLI");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("task context must be rejected");
    assert!(error
        .to_string()
        .contains("daemon logs is not available inside a daemon-managed task"));
}

#[tokio::test]
async fn daemon_stop_is_rejected_inside_a_daemon_task_before_control_access() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_TASK_ID", "task-1");
    let cli = Cli::try_parse_from(["patchbay", "daemon", "stop"]).expect("daemon stop CLI");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("task context must be rejected");
    assert!(error
        .to_string()
        .contains("daemon stop is not available inside a daemon-managed task"));
}

#[tokio::test]
async fn daemon_restart_is_rejected_inside_a_daemon_task_before_control_access() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_TASK_ID", "task-1");
    let cli = Cli::try_parse_from(["patchbay", "daemon", "restart"]).expect("daemon restart CLI");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("task context must be rejected");
    assert!(error
        .to_string()
        .contains("daemon restart is not available inside a daemon-managed task"));
}
