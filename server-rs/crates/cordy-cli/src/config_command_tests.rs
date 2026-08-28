use super::*;
use clap::Parser;
use std::fs;
use std::io::Cursor;
use std::path::Path;

#[test]
fn config_agent_timeout_display_preserves_three_states() {
    let path = Path::new("/tmp/config.json");

    let disabled = format_config_table(path, "", &[("agent_timeout", Value::String("0s".into()))]);
    assert!(disabled.contains("0s (disabled)"));

    let positive = format_config_table(path, "", &[("agent_timeout", Value::String("30m".into()))]);
    assert!(positive.contains("30m"));
    assert!(!positive.contains("disabled"));

    let unset = format_config_table(path, "", &[("agent_timeout", Value::Null)]);
    assert!(unset.contains("(not set)"));
}

#[tokio::test]
async fn config_show_table_and_json_exclude_credentials_and_unknown_fields() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let profile_path = home.path().join(".cordy/profiles/dev/config.json");
    fs::create_dir_all(profile_path.parent().expect("profile parent")).expect("profile dir");
    fs::write(
        &profile_path,
        r#"{
  "server_url": "https://api.example.com",
  "workspace_id": "workspace-1",
  "agent_timeout": "0s",
  "disable_auto_update": true,
  "token": "pby_secret",
  "future_secret": "do-not-print"
}"#,
    )
    .expect("profile config");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());

    let table = Cli::try_parse_from(["cordy", "--profile", "dev", "config"])
        .expect("config default-show CLI");
    let output = run_with_input(&table, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("config table");
    assert!(output.stdout.contains("Profile:      dev"));
    assert!(output.stdout.contains("agent_timeout:"));
    assert!(output.stdout.contains("0s (disabled)"));
    assert!(output.stdout.contains("disable_auto_update:"));
    assert!(!output.stdout.contains("pby_secret"));
    assert!(!output.stdout.contains("do-not-print"));

    let json = Cli::try_parse_from([
        "cordy",
        "--profile",
        "dev",
        "config",
        "show",
        "--output",
        "json",
    ])
    .expect("config JSON CLI");
    let output = run_with_input(&json, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("config JSON");
    let config: Value = serde_json::from_str(&output.stdout).expect("config JSON output");
    assert_eq!(config["profile"], "dev");
    assert_eq!(config["server_url"], "https://api.example.com");
    assert_eq!(config["disable_auto_update"], true);
    assert!(config.get("token").is_none());
    assert!(config.get("future_secret").is_none());
}

#[tokio::test]
async fn config_set_is_profile_scoped_and_preserves_unrelated_fields() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let default_path = home.path().join(".cordy/config.json");
    let profile_path = home.path().join(".cordy/profiles/dev/config.json");
    fs::create_dir_all(default_path.parent().expect("default parent")).expect("default dir");
    fs::create_dir_all(profile_path.parent().expect("profile parent")).expect("profile dir");
    let default_bytes = br#"{"server_url":"https://default.example","token":"pby_default"}"#;
    fs::write(&default_path, default_bytes).expect("default config");
    fs::write(
        &profile_path,
        r#"{"token":"pby_dev","future":{"keep":true}}"#,
    )
    .expect("profile config");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());

    for (key, value, expected) in [
        (
            "server_url",
            "https://api.dev.example",
            "https://api.dev.example",
        ),
        ("heartbeat_interval", " 5s ", "5s"),
        ("max_concurrent_tasks", "4", "4"),
        ("disable_auto_reload", "true", "true"),
    ] {
        let cli = Cli::try_parse_from(["cordy", "--profile", "dev", "config", "set", key, value])
            .expect("config set CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("config set");
        assert_eq!(output.stderr, format!("Set {key} = {expected}\n"));
    }
    let saved: Value = serde_json::from_slice(&fs::read(&profile_path).expect("saved profile"))
        .expect("saved JSON");
    assert_eq!(saved["token"], "pby_dev");
    assert_eq!(saved["future"]["keep"], true);
    assert_eq!(saved["heartbeat_interval"], "5s");
    assert_eq!(saved["max_concurrent_tasks"], 4);
    assert_eq!(saved["disable_auto_reload"], true);
    assert_eq!(
        fs::read(&default_path).expect("default unchanged"),
        default_bytes
    );
}

#[test]
fn config_set_whitelist_and_validation_match_registry_contract() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let root = cwd.path().join("data/cordy").display().to_string();
    let valid = [
        ("server_url", "https://api.example.com"),
        ("app_url", "https://app.example.com"),
        ("workspace_id", "workspace-1"),
        ("device_name", "host-a"),
        ("runtime_name", "runtime-a"),
        ("workspaces_root", "data/cordy"),
        ("max_concurrent_tasks", "8"),
        ("poll_interval", "1m30s"),
        ("heartbeat_interval", " 5s "),
        ("agent_timeout", "0s"),
        ("codex_semantic_inactivity_timeout", "15m"),
        ("codex_handshake_timeout", "45s"),
        ("disable_auto_update", "TRUE"),
        ("auto_update_check_interval", "12h"),
        ("disable_auto_reload", "false"),
    ];
    for (key, value) in valid {
        let (_, displayed) =
            validate_config_set(key, value, &environment).expect("valid config value");
        if key == "workspaces_root" {
            assert_eq!(displayed, root);
        }
    }
    for (key, value, message) in [
        ("token", "secret", "unknown config key"),
        ("server_url", "not a URL", "valid URL"),
        ("app_url", "ftp://example.com", "must use one of"),
        ("max_concurrent_tasks", "-1", ">= 0"),
        ("poll_interval", "0s", "positive"),
        ("heartbeat_interval", "abc", "duration"),
        ("agent_timeout", "-1s", ">= 0"),
        ("disable_auto_update", "maybe", "true"),
    ] {
        assert!(validate_config_set(key, value, &environment)
            .expect_err("invalid config value")
            .to_string()
            .contains(message));
    }
}

#[tokio::test]
async fn config_commands_fail_closed_without_task_local_root() {
    let home = tempfile::tempdir().expect("owner home");
    let cwd = tempfile::tempdir().expect("task cwd");
    let owner_path = home.path().join(".cordy/config.json");
    fs::create_dir_all(owner_path.parent().expect("owner parent")).expect("owner dir");
    let owner_bytes = br#"{"server_url":"https://owner.invalid","token":"pby_owner"}"#;
    fs::write(&owner_path, owner_bytes).expect("owner config");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_AGENT_ID", "agent-1");
    environment.set("CORDY_TASK_ID", "task-1");
    let cli = Cli::try_parse_from([
        "cordy",
        "config",
        "set",
        "server_url",
        "https://task.example",
    ])
    .expect("task config set CLI");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("missing task root");
    assert!(error.to_string().contains("task-local Cordy config root"));
    assert_eq!(fs::read(&owner_path).expect("owner unchanged"), owner_bytes);

    let task_root = tempfile::tempdir().expect("task root");
    environment.set(
        config::TASK_CONFIG_ROOT_ENV,
        task_root.path().display().to_string(),
    );
    run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect("task-local config set");
    let task: Value = serde_json::from_slice(
        &fs::read(task_root.path().join("config.json")).expect("task config"),
    )
    .expect("task config JSON");
    assert_eq!(task["server_url"], "https://task.example");
    assert_eq!(
        fs::read(&owner_path).expect("owner still unchanged"),
        owner_bytes
    );
}
