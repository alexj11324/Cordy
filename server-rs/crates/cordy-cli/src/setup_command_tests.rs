use super::*;
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use clap::Parser;
use std::fs;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[test]
fn setup_parser_exposes_cloud_and_self_host_boundaries() {
    assert!(matches!(
        Cli::try_parse_from(["cordy", "setup"])
            .expect("cloud setup")
            .command,
        Command::Setup(SetupArgs {
            callback_host: None,
            command: None,
        })
    ));

    let cli = Cli::try_parse_from([
        "cordy",
        "--profile",
        "staging",
        "--server-url",
        "wss://api.example/ws",
        "setup",
        "self-host",
        "--app-url",
        "https://app.example/",
        "--port",
        "9090",
        "--frontend-port",
        "4000",
    ])
    .expect("self-host setup");
    let Command::Setup(SetupArgs {
        command: Some(SetupCommand::SelfHost(options)),
        ..
    }) = cli.command
    else {
        panic!("expected self-host setup");
    };
    assert_eq!(options.app_url.as_deref(), Some("https://app.example/"));
    assert_eq!(options.port, 9090);
    assert_eq!(options.frontend_port, 4000);
}

#[test]
fn setup_callback_host_is_available_for_cloud_and_self_host_browser_flows() {
    let cloud = Cli::try_parse_from(["cordy", "setup", "cloud", "--callback-host", "192.168.1.20"])
        .expect("cloud callback host");
    let Command::Setup(SetupArgs {
        command: Some(command),
        ..
    }) = cloud.command
    else {
        panic!("expected cloud setup");
    };
    let SetupCommand::Cloud(options) = command else {
        panic!("expected cloud options");
    };
    assert_eq!(options.callback_host.as_deref(), Some("192.168.1.20"));
    assert_eq!(
        setup_callback_host(&SetupArgs {
            callback_host: None,
            command: Some(SetupCommand::Cloud(options)),
        }),
        Some("192.168.1.20".into())
    );

    let self_host =
        Cli::try_parse_from(["cordy", "setup", "self-host", "--callback-host", "10.0.0.7"])
            .expect("self-host callback host");
    let Command::Setup(SetupArgs {
        command: Some(command),
        ..
    }) = self_host.command
    else {
        panic!("expected self-host setup");
    };
    let SetupCommand::SelfHost(options) = command else {
        panic!("expected self-host options");
    };
    assert_eq!(options.callback_host.as_deref(), Some("10.0.0.7"));
    assert_eq!(
        setup_callback_host(&SetupArgs {
            callback_host: None,
            command: Some(SetupCommand::SelfHost(options)),
        }),
        Some("10.0.0.7".into())
    );

    let root = Cli::try_parse_from(["cordy", "setup", "--callback-host", "localhost"])
        .expect("root callback host");
    let Command::Setup(root_args) = root.command else {
        panic!("expected root setup");
    };
    assert_eq!(
        setup_callback_host(&root_args).as_deref(),
        Some("localhost")
    );
}

#[test]
fn setup_remote_self_host_requires_explicit_app_url() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let cli = Cli::try_parse_from([
        "cordy",
        "--server-url",
        "https://api.example",
        "setup",
        "self-host",
    ])
    .expect("setup CLI");
    let Command::Setup(args) = &cli.command else {
        panic!("expected setup");
    };
    let error = resolve_setup_profile_input(&cli, &environment, args)
        .expect_err("remote setup must not guess app URL");
    assert!(error.to_string().contains("requires --app-url"));
}

#[test]
fn setup_overwrite_confirmation_accepts_yes() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let profile = "staging";
    let config_path = environment.config_path(profile).expect("config path");
    fs::create_dir_all(config_path.parent().expect("config parent")).expect("config dir");
    fs::write(
        &config_path,
        r#"{"server_url":"https://old.example","app_url":"https://old.app","token":"secret","workspace_id":"workspace-1"}"#,
    )
    .expect("old config");
    let cli = Cli::try_parse_from(["cordy", "--profile", profile, "setup"]).expect("setup CLI");
    let target = config::SetupProfileInput::new("https://new.example", "https://new.app")
        .expect("target profile");
    let mut input = Cursor::new(b"YES\n".to_vec());

    assert!(
        confirm_setup_overwrite(&cli, &environment, &target, &mut input,).expect("confirmation")
    );
}

#[test]
fn setup_overwrite_confirmation_rejects_without_mutating_profile() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let profile = "staging";
    let config_path = environment.config_path(profile).expect("config path");
    fs::create_dir_all(config_path.parent().expect("config parent")).expect("config dir");
    let old_config = br#"{"server_url":"https://old.example","app_url":"https://old.app","token":"secret","workspace_id":"workspace-1"}"#;
    fs::write(&config_path, old_config).expect("old config");
    let lock_path = config_path
        .parent()
        .expect("config parent")
        .join(".config.lock");
    let cli = Cli::try_parse_from(["cordy", "--profile", profile, "setup"]).expect("setup CLI");
    let target = config::SetupProfileInput::new("https://new.example", "https://new.app")
        .expect("target profile");
    let mut input = Cursor::new(b"n\n".to_vec());

    assert!(
        !confirm_setup_overwrite(&cli, &environment, &target, &mut input,).expect("confirmation")
    );
    assert_eq!(fs::read(&config_path).expect("config remains"), old_config);
    assert!(!lock_path.exists(), "declining must not create the lock");
}

#[tokio::test]
async fn setup_decline_returns_aborted_before_health_or_daemon_handoff() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let profile = "staging";
    let config_path = environment.config_path(profile).expect("config path");
    fs::create_dir_all(config_path.parent().expect("config parent")).expect("config dir");
    let old_config =
        br#"{"server_url":"https://old.example","token":"secret","workspace_id":"workspace-1"}"#;
    fs::write(&config_path, old_config).expect("old config");
    let cli = Cli::try_parse_from(["cordy", "--profile", profile, "setup"]).expect("setup CLI");
    let mut input = Cursor::new(b"n\n".to_vec());

    let output = run_with_input(&cli, &environment, &mut input)
        .await
        .expect("declined setup");
    assert_eq!(output.stderr, "Aborted.\n");
    assert_eq!(fs::read(config_path).expect("config remains"), old_config);
}

#[test]
fn setup_overwrite_confirmation_eof_defaults_to_reject() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let profile = "staging";
    let config_path = environment.config_path(profile).expect("config path");
    fs::create_dir_all(config_path.parent().expect("config parent")).expect("config dir");
    fs::write(
        &config_path,
        r#"{"server_url":"https://old.example","app_url":"https://old.app"}"#,
    )
    .expect("old config");
    let cli = Cli::try_parse_from(["cordy", "--profile", profile, "setup"]).expect("setup CLI");
    let target = config::SetupProfileInput::new("https://new.example", "https://new.app")
        .expect("target profile");
    let mut input = Cursor::new(Vec::<u8>::new());

    assert!(
        !confirm_setup_overwrite(&cli, &environment, &target, &mut input,).expect("confirmation")
    );
}

#[test]
fn setup_overwrite_confirmation_skips_prompt_without_existing_profile() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let cli = Cli::try_parse_from(["cordy", "setup"]).expect("setup CLI");
    let target = config::SetupProfileInput::new("https://new.example", "https://new.app")
        .expect("target profile");
    let mut input = Cursor::new(b"not-consumed\n".to_vec());

    assert!(
        confirm_setup_overwrite(&cli, &environment, &target, &mut input,).expect("confirmation")
    );
    assert_eq!(input.position(), 0, "fresh setup must not prompt");
}

#[tokio::test]
async fn setup_probes_before_replacing_profile_and_preserves_env_token() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let profile = "staging";
    let config_path = environment.config_path(profile).expect("config path");
    fs::create_dir_all(config_path.parent().expect("config parent")).expect("config dir");
    let old_config = br#"{"server_url":"http://old.example","app_url":"http://old.app","token":"mul_old","workspace_id":"old"}"#;
    fs::write(&config_path, old_config).expect("old config");

    let observed_during_probe = Arc::new(Mutex::new(None::<Vec<u8>>));
    let observed = Arc::clone(&observed_during_probe);
    let path_for_server = config_path.clone();
    let app = Router::new().route(
        "/health",
        get(move || {
            let observed = Arc::clone(&observed);
            let path = path_for_server.clone();
            async move {
                *observed.lock().expect("probe capture") = fs::read(path).ok();
                StatusCode::OK
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });

    let mut environment = environment;
    environment.set("CORDY_TOKEN", "mul_env");
    let cli = Cli::try_parse_from([
        "cordy",
        "--profile",
        profile,
        "--server-url",
        &format!("http://{address}"),
        "setup",
        "self-host",
        "--app-url",
        "https://app.example/",
    ])
    .expect("setup CLI");
    let Command::Setup(args) = &cli.command else {
        panic!("expected setup command");
    };
    let input = prepare_setup_profile(&cli, &environment, args)
        .await
        .expect("setup profile should persist with env token");
    assert_eq!(
        *observed_during_probe.lock().expect("probe capture"),
        Some(old_config.to_vec())
    );
    let saved: Value =
        serde_json::from_slice(&fs::read(config_path).expect("saved config")).expect("saved JSON");
    assert_eq!(saved["server_url"], format!("http://{address}"));
    assert_eq!(saved["app_url"], "https://app.example");
    assert_eq!(saved["token"], "mul_env");
    assert!(saved.get("workspace_id").is_none());
    assert_eq!(input.server_url, format!("http://{address}"));
    server.abort();
}

#[tokio::test]
async fn setup_failed_probe_does_not_mutate_existing_profile() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    let profile = "staging";
    let config_path = environment.config_path(profile).expect("config path");
    fs::create_dir_all(config_path.parent().expect("config parent")).expect("config dir");
    let old_config =
        br#"{"server_url":"http://old.example","token":"mul_old","workspace_id":"old"}"#;
    fs::write(&config_path, old_config).expect("old config");
    environment.set("CORDY_TOKEN", "mul_env");
    let cli = Cli::try_parse_from([
        "cordy",
        "--profile",
        profile,
        "--server-url",
        "http://127.0.0.1:1",
        "setup",
        "self-host",
        "--app-url",
        "https://app.example",
    ])
    .expect("setup CLI");
    let Command::Setup(args) = &cli.command else {
        panic!("expected setup command");
    };
    let error = prepare_setup_profile(&cli, &environment, args)
        .await
        .expect_err("unreachable setup target");
    assert!(error.to_string().contains("health preflight failed"));
    assert_eq!(fs::read(config_path).expect("config remains"), old_config);
}

#[test]
fn setup_daemon_action_respects_active_work_and_presence() {
    assert_eq!(setup_daemon_action(false, 0), SetupDaemonAction::Start);
    assert_eq!(setup_daemon_action(true, 0), SetupDaemonAction::Restart);
    assert_eq!(
        setup_daemon_action(true, 2),
        SetupDaemonAction::LeaveRunning {
            active_task_count: 2
        }
    );
}

#[tokio::test]
async fn setup_daemon_dispatch_propagates_start_restart_failures() {
    let started = dispatch_daemon_after_setup(
        SetupDaemonAction::Start,
        || async {
            Ok(RunOutput {
                stdout: String::new(),
                stderr: String::new(),
            })
        },
        || async {
            Ok(RunOutput {
                stdout: String::new(),
                stderr: String::new(),
            })
        },
    )
    .await
    .expect("start dispatch");
    assert!(started.stdout.is_empty());

    let restart_error = dispatch_daemon_after_setup(
        SetupDaemonAction::Restart,
        || async {
            Ok(RunOutput {
                stdout: String::new(),
                stderr: String::new(),
            })
        },
        || async { Err(anyhow::anyhow!("restart failed")) },
    )
    .await
    .expect_err("restart failure must be visible");
    assert_eq!(restart_error.to_string(), "restart failed");
}

#[tokio::test]
async fn setup_daemon_dispatch_leaves_active_daemon_untouched() {
    let error = dispatch_daemon_after_setup(
        SetupDaemonAction::LeaveRunning {
            active_task_count: 1,
        },
        || async { panic!("setup must not start over active work") },
        || async { panic!("setup must not restart over active work") },
    )
    .await
    .expect_err("active work must defer restart");
    assert!(error.to_string().contains("1 active task"));
}

#[test]
fn websocket_server_urls_normalize_to_http_api_base() {
    assert_eq!(
        normalize_api_base_url("wss://api.cordy.ai/ws?old=1#fragment").expect("URL"),
        "https://api.cordy.ai"
    );
}
