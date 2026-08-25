//! Profile schema and daemon launch resolution.
//!
//! Environment capture and atomic persistence live in `config_environment`;
//! profile schema and launch precedence live in `config_profile`.

mod config_environment;
mod config_persistence;
mod config_profile_resolution;
mod config_profile_schema;

pub use config_environment::{Environment, SetupProfileInput, TASK_CONFIG_ROOT_ENV};
pub use config_profile_resolution::resolve_daemon_launch_overrides;
pub use config_profile_schema::{
    BackendOverrides, CliConfig, DaemonLaunchFlags, OpenClawOverride,
};

use serde_json::Value;
use std::fs;
use std::time::Duration;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_profile_schema_preserves_persisted_launch_and_backend_inputs() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let path = environment.config_path("production").expect("config path");
        fs::create_dir_all(path.parent().expect("profile directory")).expect("create profile");
        fs::write(
            &path,
            r#"{
                "server_url":"https://cordy.example",
                "app_url":"https://app.cordy.example",
                "workspace_id":"workspace-1",
                "token":"mul_secret",
                "device_name":"build-host",
                "runtime_name":"night-shift",
                "workspaces_root":"/srv/cordy-workspaces",
                "max_concurrent_tasks":7,
                "poll_interval":"3s",
                "heartbeat_interval":"11s",
                "agent_timeout":"0s",
                "codex_semantic_inactivity_timeout":"17m",
                "codex_handshake_timeout":"42s",
                "disable_auto_update":true,
                "auto_update_check_interval":"4h",
                "disable_auto_reload":true,
                "backends":{"openclaw":{
                    "binary_path":"/opt/openclaw/bin/openclaw",
                    "state_dir":"/srv/openclaw-state",
                    "cli_timeout":"45s"
                }},
                "profile_command_overrides":{
                    "profile-1":"/opt/agents/custom-codex"
                }
            }"#,
        )
        .expect("write profile");

        let config = environment.load_config("production").expect("load profile");
        assert_eq!(config.server_url, "https://cordy.example");
        assert_eq!(config.app_url, "https://app.cordy.example");
        assert_eq!(config.workspace_id, "workspace-1");
        assert_eq!(config.device_name, "build-host");
        assert_eq!(config.runtime_name, "night-shift");
        assert_eq!(config.workspaces_root, "/srv/cordy-workspaces");
        assert_eq!(config.max_concurrent_tasks, 7);
        assert_eq!(config.poll_interval, "3s");
        assert_eq!(config.heartbeat_interval, "11s");
        assert_eq!(config.agent_timeout.as_deref(), Some("0s"));
        assert_eq!(config.codex_semantic_inactivity_timeout, "17m");
        assert_eq!(config.codex_handshake_timeout, "42s");
        assert!(config.disable_auto_update);
        assert_eq!(config.auto_update_check_interval, "4h");
        assert!(config.disable_auto_reload);

        let daemon = config.daemon_profile_input();
        assert_eq!(daemon.token, "mul_secret");
        assert_eq!(
            daemon
                .profile_command_overrides
                .get("profile-1")
                .map(String::as_str),
            Some("/opt/agents/custom-codex")
        );
        assert_eq!(daemon.openclaw_binary_path, "/opt/openclaw/bin/openclaw");
        assert_eq!(daemon.openclaw_state_dir, "/srv/openclaw-state");
        assert_eq!(daemon.openclaw_cli_timeout, "45s");
    }

    #[test]
    fn absent_agent_timeout_and_backend_overrides_remain_unset() {
        let config: CliConfig =
            serde_json::from_str(r#"{"token":"mul_secret"}"#).expect("minimal profile");
        assert!(config.agent_timeout.is_none());
        assert!(config.backends.is_none());

        let daemon = config.daemon_profile_input();
        assert!(daemon.openclaw_binary_path.is_empty());
        assert!(daemon.openclaw_state_dir.is_empty());
        assert!(daemon.openclaw_cli_timeout.is_empty());
    }

    #[test]
    fn daemon_launch_resolver_applies_persisted_values_when_flag_and_env_are_absent() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let config: CliConfig = serde_json::from_str(
            r#"{
                "server_url":"https://profile.example",
                "device_name":"profile-device",
                "runtime_name":"profile-runtime",
                "workspaces_root":"/profile/workspaces",
                "max_concurrent_tasks":9,
                "poll_interval":"3s",
                "heartbeat_interval":"11s",
                "agent_timeout":"0s",
                "codex_semantic_inactivity_timeout":"17m",
                "codex_handshake_timeout":"42s",
                "disable_auto_update":true,
                "auto_update_check_interval":"4h",
                "disable_auto_reload":true
            }"#,
        )
        .expect("profile config");

        let resolved = resolve_daemon_launch_overrides(
            "production",
            &DaemonLaunchFlags::default(),
            &environment,
            &config,
        )
        .expect("resolve launch");
        assert_eq!(resolved.server_url, "https://profile.example");
        assert_eq!(resolved.device_name, "profile-device");
        assert_eq!(resolved.runtime_name, "profile-runtime");
        assert_eq!(resolved.workspaces_root, "/profile/workspaces");
        assert_eq!(resolved.max_concurrent_tasks, 9);
        assert_eq!(resolved.poll_interval, Duration::from_secs(3));
        assert_eq!(resolved.heartbeat_interval, Duration::from_secs(11));
        assert_eq!(resolved.agent_timeout, Some(Duration::ZERO));
        assert_eq!(
            resolved.codex_semantic_inactivity_timeout,
            Duration::from_secs(17 * 60)
        );
        assert_eq!(resolved.codex_handshake_timeout, Duration::from_secs(42));
        assert!(resolved.disable_auto_update);
        assert_eq!(
            resolved.auto_update_check_interval,
            Duration::from_secs(4 * 60 * 60)
        );
        assert!(resolved.disable_auto_reload);
        assert_eq!(resolved.profile, "production");
        assert_eq!(
            resolved.health_port,
            i32::from(cordy_daemon::control_client::health_port_for_profile(
                "production"
            ))
        );
    }

    #[test]
    fn daemon_launch_resolver_leaves_environment_values_to_daemon_config() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        for (key, value) in [
            ("CORDY_SERVER_URL", "https://env.example"),
            ("CORDY_DAEMON_ID", "env-daemon"),
            ("CORDY_DAEMON_DEVICE_NAME", "env-device"),
            ("CORDY_AGENT_RUNTIME_NAME", "env-runtime"),
            ("CORDY_WORKSPACES_ROOT", "/env/workspaces"),
            ("CORDY_DAEMON_MAX_CONCURRENT_TASKS", "5"),
            ("CORDY_DAEMON_POLL_INTERVAL", "5s"),
            ("CORDY_DAEMON_HEARTBEAT_INTERVAL", "13s"),
            ("CORDY_AGENT_TIMEOUT", "2h"),
            ("CORDY_CODEX_SEMANTIC_INACTIVITY_TIMEOUT", "19m"),
            ("CORDY_CODEX_HANDSHAKE_TIMEOUT", "51s"),
            ("CORDY_DAEMON_AUTO_UPDATE_INTERVAL", "6h"),
            ("CORDY_DAEMON_AUTO_UPDATE", "true"),
            ("CORDY_DAEMON_AUTO_RELOAD", "false"),
        ] {
            environment.set(key, value);
        }
        let config: CliConfig = serde_json::from_str(
            r#"{
                "server_url":"https://profile.example",
                "device_name":"profile-device",
                "runtime_name":"profile-runtime",
                "workspaces_root":"/profile/workspaces",
                "max_concurrent_tasks":9,
                "poll_interval":"3s",
                "heartbeat_interval":"11s",
                "agent_timeout":"0s",
                "codex_semantic_inactivity_timeout":"17m",
                "codex_handshake_timeout":"42s",
                "disable_auto_update":true,
                "auto_update_check_interval":"4h",
                "disable_auto_reload":false
            }"#,
        )
        .expect("profile config");

        let resolved = resolve_daemon_launch_overrides(
            "",
            &DaemonLaunchFlags::default(),
            &environment,
            &config,
        )
        .expect("resolve launch");
        assert_eq!(resolved.server_url, "https://env.example");
        assert!(resolved.daemon_id.is_empty());
        assert!(resolved.device_name.is_empty());
        assert!(resolved.runtime_name.is_empty());
        assert!(resolved.workspaces_root.is_empty());
        assert_eq!(resolved.max_concurrent_tasks, 0);
        assert!(resolved.poll_interval.is_zero());
        assert!(resolved.heartbeat_interval.is_zero());
        assert!(resolved.agent_timeout.is_none());
        assert!(resolved.codex_semantic_inactivity_timeout.is_zero());
        assert!(resolved.codex_handshake_timeout.is_zero());
        assert!(resolved.auto_update_check_interval.is_zero());
        assert!(!resolved.disable_auto_update);
        assert!(resolved.disable_auto_reload);
    }

    #[test]
    fn daemon_launch_flags_win_and_preserve_explicit_zero_agent_timeout() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", "https://env.example");
        environment.set("CORDY_AGENT_TIMEOUT", "2h");
        environment.set("CORDY_DAEMON_AUTO_UPDATE", "true");
        let flags = DaemonLaunchFlags {
            server_url: Some("https://flag.example".into()),
            daemon_id: Some("flag-daemon".into()),
            device_name: Some("flag-device".into()),
            runtime_name: Some("flag-runtime".into()),
            workspaces_root: Some("/flag/workspaces".into()),
            poll_interval: Some(Duration::from_secs(2)),
            heartbeat_interval: Some(Duration::from_secs(7)),
            agent_timeout: Some(Duration::ZERO),
            codex_semantic_inactivity_timeout: Some(Duration::from_secs(23 * 60)),
            codex_handshake_timeout: Some(Duration::from_secs(61)),
            max_concurrent_tasks: Some(12),
            disable_auto_update: true,
            auto_update_check_interval: Some(Duration::from_secs(8 * 60 * 60)),
            disable_auto_reload: true,
        };
        let resolved = resolve_daemon_launch_overrides(
            "flag-profile",
            &flags,
            &environment,
            &CliConfig::default(),
        )
        .expect("resolve launch");
        assert_eq!(resolved.server_url, "https://flag.example");
        assert_eq!(resolved.daemon_id, "flag-daemon");
        assert_eq!(resolved.device_name, "flag-device");
        assert_eq!(resolved.runtime_name, "flag-runtime");
        assert_eq!(resolved.workspaces_root, "/flag/workspaces");
        assert_eq!(resolved.poll_interval, Duration::from_secs(2));
        assert_eq!(resolved.heartbeat_interval, Duration::from_secs(7));
        assert_eq!(resolved.agent_timeout, Some(Duration::ZERO));
        assert_eq!(resolved.max_concurrent_tasks, 12);
        assert!(resolved.disable_auto_update);
        assert!(resolved.disable_auto_reload);
    }

    #[test]
    fn invalid_persisted_daemon_duration_fails_closed() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let config = CliConfig {
            poll_interval: "eventually".into(),
            ..CliConfig::default()
        };
        let error = resolve_daemon_launch_overrides(
            "",
            &DaemonLaunchFlags::default(),
            &environment,
            &config,
        )
        .expect_err("invalid duration must fail");
        let message = format!("{error:#}");
        assert!(message.contains("CORDY_DAEMON_POLL_INTERVAL"));
        assert!(message.contains("eventually"));
    }

    #[test]
    fn profile_paths_match_go_layouts() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut env = Environment::for_test(home.path().into(), cwd.path().into());

        assert_eq!(
            env.config_path("").expect("default path"),
            home.path().join(".cordy/config.json")
        );
        assert_eq!(
            env.config_path("dev").expect("profile path"),
            home.path().join(".cordy/profiles/dev/config.json")
        );

        let task_root = home.path().join("task-config");
        env.set(TASK_CONFIG_ROOT_ENV, task_root.display().to_string());
        assert_eq!(
            env.config_path("dev").expect("task profile path"),
            task_root.join("profiles/dev/config.json")
        );
        assert!(env.config_path("../owner").is_err());
    }

    #[test]
    fn task_marker_is_fail_closed_and_actionable_only_when_task_scoped() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let marker_dir = cwd.path().join(".cordy");
        fs::create_dir_all(&marker_dir).expect("marker dir");
        let marker_path = marker_dir.join("daemon_task_context.json");
        fs::write(
            &marker_path,
            r#"{"managed_by":"cordy-daemon-task","agent_id":"agent-1"}"#,
        )
        .expect("marker");

        let env = Environment::for_test(home.path().into(), cwd.path().into());
        assert!(env.in_daemon_managed_execution_context());
        assert!(env
            .leftover_marker_suffix()
            .expect("leftover suffix")
            .contains(marker_path.to_string_lossy().as_ref()));

        fs::write(&marker_path, r#"{"managed_by":"cordy-daemon-task"}"#).expect("root marker");
        assert!(env.leftover_marker_suffix().is_none());
    }

    #[test]
    fn clear_profile_token_is_locked_atomic_and_preserves_other_fields() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let profile_dir = home.path().join(".cordy/profiles/dev");
        fs::create_dir_all(&profile_dir).expect("profile dir");
        fs::write(
            profile_dir.join("config.json"),
            r#"{"server_url":"https://dev.example","token":"mul_dev","future":{"enabled":true}}"#,
        )
        .expect("profile config");
        fs::write(profile_dir.join(".config.lock"), b"lock-sentinel").expect("lock sentinel");
        let default_path = home.path().join(".cordy/config.json");
        fs::create_dir_all(default_path.parent().expect("default dir")).expect("default dir");
        let default_bytes = br#"{"token":"mul_default","workspace_id":"default-workspace"}"#;
        fs::write(&default_path, default_bytes).expect("default config");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());

        let outcomes = std::thread::scope(|scope| {
            let first = environment.clone();
            let second = environment.clone();
            let first = scope.spawn(move || first.clear_profile_token("dev").expect("first clear"));
            let second =
                scope.spawn(move || second.clear_profile_token("dev").expect("second clear"));
            [
                first.join().expect("first thread"),
                second.join().expect("second thread"),
            ]
        });
        assert_eq!(outcomes.into_iter().filter(|removed| *removed).count(), 1);

        let saved: Value = serde_json::from_slice(
            &fs::read(profile_dir.join("config.json")).expect("saved profile"),
        )
        .expect("saved JSON");
        assert!(saved.get("token").is_none());
        assert_eq!(saved["server_url"], "https://dev.example");
        assert_eq!(saved["future"]["enabled"], true);
        assert_eq!(
            fs::read(profile_dir.join(".config.lock")).expect("lock file"),
            b"lock-sentinel"
        );
        assert_eq!(
            fs::read(&default_path).expect("default unchanged"),
            default_bytes
        );
        assert!(!environment
            .clear_profile_token("missing")
            .expect("missing profile is idempotent"));
    }

    #[test]
    fn set_profile_value_does_not_truncate_existing_lock_file() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let profile_dir = home.path().join(".cordy/profiles/dev");
        fs::create_dir_all(&profile_dir).expect("profile dir");
        fs::write(profile_dir.join(".config.lock"), b"lock-sentinel").expect("lock sentinel");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());

        environment
            .set_profile_value(
                "dev",
                "workspace_id",
                Some(Value::String("workspace-1".into())),
            )
            .expect("set profile value");

        assert_eq!(
            fs::read(profile_dir.join(".config.lock")).expect("lock file"),
            b"lock-sentinel"
        );
    }

    #[test]
    fn setup_health_failure_does_not_mutate_existing_profile() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let profile_dir = home.path().join(".cordy/profiles/dev");
        fs::create_dir_all(&profile_dir).expect("profile dir");
        let config_path = profile_dir.join("config.json");
        let old_config = br#"{
            "server_url":"https://old.example",
            "app_url":"https://old-app.example",
            "token":"mul_old",
            "workspace_id":"workspace-old",
            "future":{"kept":true}
        }
        "#;
        fs::write(&config_path, old_config).expect("old config");
        fs::write(profile_dir.join(".config.lock"), b"lock-sentinel").expect("lock sentinel");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let input = SetupProfileInput::new("https://new.example", "https://new-app.example")
            .expect("setup input");

        let persisted = environment
            .replace_profile_for_setup_if_reachable("dev", &input, |_| false)
            .expect("probe failure is not an I/O error");

        assert!(!persisted);
        assert_eq!(fs::read(&config_path).expect("config remains"), old_config);
        assert_eq!(
            fs::read(profile_dir.join(".config.lock")).expect("lock remains"),
            b"lock-sentinel"
        );
    }

    #[test]
    fn setup_success_replaces_whole_profile_atomically() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let profile_dir = home.path().join(".cordy/profiles/dev");
        fs::create_dir_all(&profile_dir).expect("profile dir");
        fs::write(
            profile_dir.join("config.json"),
            br#"{"server_url":"https://old.example","token":"mul_old","workspace_id":"old","future":{"kept":true}}"#,
        )
        .expect("old config");
        fs::write(profile_dir.join(".config.lock"), b"lock-sentinel").expect("lock sentinel");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let input = SetupProfileInput::new("https://new.example", "https://new-app.example")
            .expect("setup input");

        assert!(environment
            .replace_profile_for_setup_if_reachable("dev", &input, |url| {
                assert_eq!(url, "https://new.example");
                true
            })
            .expect("persist setup profile"));

        let saved = environment
            .load_profile_document("dev")
            .expect("saved config");
        assert_eq!(saved["server_url"], "https://new.example");
        assert_eq!(saved["app_url"], "https://new-app.example");
        assert!(saved.get("token").is_none());
        assert!(saved.get("workspace_id").is_none());
        assert!(saved.get("future").is_none());
        assert_eq!(
            fs::read(profile_dir.join(".config.lock")).expect("lock remains"),
            b"lock-sentinel"
        );
    }

    #[test]
    fn setup_input_rejects_empty_urls() {
        assert!(SetupProfileInput::new("", "https://app.example").is_err());
        assert!(SetupProfileInput::new("https://api.example", " ").is_err());
    }
}
