use super::*;
use crate::update_commands::write_update_progress;
use clap::Parser;
use serde_json::Value;
use std::io::Cursor;
use std::time::Duration;

#[test]
fn version_text_json_and_root_flag_match_release_contract() {
    let text = run_version(VersionOutput::Text).expect("text version");
    assert_eq!(
        text.stdout,
        format!(
            "patchbay {CLIENT_VERSION} (commit: {BUILD_COMMIT}, built: {BUILD_DATE})\nos/arch: {BUILD_OS}/{BUILD_ARCH}\n"
        )
    );
    assert!(text.stderr.is_empty());

    let json = run_version(VersionOutput::Json).expect("JSON version");
    let info: Value = serde_json::from_str(&json.stdout).expect("version JSON");
    assert_eq!(info.as_object().expect("version object").len(), 5);
    assert_eq!(info["version"], CLIENT_VERSION);
    assert_eq!(info["commit"], BUILD_COMMIT);
    assert_eq!(info["date"], BUILD_DATE);
    assert_eq!(info["os"], BUILD_OS);
    assert_eq!(info["arch"], BUILD_ARCH);

    let root = Cli::try_parse_from(["patchbay", "--version"])
        .expect_err("--version exits after rendering");
    assert_eq!(root.kind(), clap::error::ErrorKind::DisplayVersion);
    assert_eq!(root.to_string(), format!("patchbay {ROOT_LONG_VERSION}\n"));
    let first_line =
        format!("patchbay {CLIENT_VERSION} (commit: {BUILD_COMMIT}, built: {BUILD_DATE})");
    assert_eq!(root.to_string().lines().next(), Some(first_line.as_str()));
}

#[test]
fn version_subcommand_accepts_only_supported_output_values() {
    assert!(Cli::try_parse_from(["patchbay", "version"]).is_ok());
    assert!(Cli::try_parse_from(["patchbay", "version", "--output", "text"]).is_ok());
    assert!(Cli::try_parse_from(["patchbay", "version", "--output", "json"]).is_ok());
    assert!(Cli::try_parse_from(["patchbay", "version", "--output", "table"]).is_err());
}

#[test]
fn completion_command_remains_hidden_but_callable_for_supported_shells() {
    use clap::CommandFactory;

    let help = Cli::command().render_help().to_string();
    assert!(!help.contains("completion"));

    for shell in ["bash", "zsh", "fish", "powershell"] {
        let cli = Cli::try_parse_from(["patchbay", "completion", shell])
            .unwrap_or_else(|error| panic!("parse {shell} completion: {error}"));
        let Command::Completion { shell } = cli.command else {
            panic!("expected completion command");
        };
        let output = run_completion(shell).expect("render completion");
        assert!(!output.stdout.trim().is_empty());
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn update_command_parses_go_timeout_and_uses_daemon_default() {
    let default = Cli::try_parse_from(["patchbay", "update"]).expect("update CLI");
    let Command::Update(args) = default.command else {
        panic!("expected update command");
    };
    assert_eq!(args.download_timeout, None);
    assert_eq!(
        resolve_update_download_timeout(&args),
        patchbay_daemon::update_executor::DEFAULT_UPDATE_DOWNLOAD_TIMEOUT
    );

    let custom = Cli::try_parse_from(["patchbay", "update", "--download-timeout", "7s"])
        .expect("custom update timeout");
    let Command::Update(args) = custom.command else {
        panic!("expected update command");
    };
    assert_eq!(args.download_timeout, Some(Duration::from_secs(7)));
    assert!(Cli::try_parse_from(["patchbay", "update", "--download-timeout", "0s"]).is_ok());
}

#[test]
fn update_output_matches_go_states_without_sensitive_details() {
    let output = render_update_outcome(patchbay_daemon::update_executor::UpdateOutcome {
        method: patchbay_daemon::update_executor::UpdateInstallMethod::Direct,
        resolved_version: Some("v1.2.4".into()),
        already_current: false,
        latest_query_failed: false,
        message: "Downloaded patchbay-cli-linux-amd64.tar.gz and replaced the current executable"
            .into(),
    });
    assert!(output.stdout.is_empty());
    assert!(output.stderr.contains("Latest version:  v1.2.4"));
    assert!(output
        .stderr
        .contains("Downloading v1.2.4 from GitHub Releases..."));
    assert!(output.stderr.contains("Update complete."));
    for forbidden in ["https://", "/home/", "token", "Authorization"] {
        assert!(!output.stderr.contains(forbidden), "leaked {forbidden}");
    }

    let current = render_update_outcome(patchbay_daemon::update_executor::UpdateOutcome {
        method: patchbay_daemon::update_executor::UpdateInstallMethod::Direct,
        resolved_version: Some("v1.2.3".into()),
        already_current: true,
        latest_query_failed: false,
        message: "Already up to date.".into(),
    });
    assert_eq!(current.stderr, "Already up to date.\n");
}

#[test]
fn update_homebrew_warning_continues_without_latest_details() {
    let output = render_update_outcome(patchbay_daemon::update_executor::UpdateOutcome {
        method: patchbay_daemon::update_executor::UpdateInstallMethod::Homebrew,
        resolved_version: None,
        already_current: false,
        latest_query_failed: true,
        message: "Homebrew upgraded a legacy Patchbay installation".into(),
    });
    assert!(output
        .stderr
        .contains("Warning: could not check latest version; continuing."));
    assert!(output.stderr.contains("Updating via Homebrew..."));
    assert!(output.stderr.contains("Update complete."));
    assert!(!output.stderr.contains("https://"));
}

#[test]
fn update_rejects_zero_timeout_before_executor_detection() {
    let error = validate_update_timeout(Duration::ZERO).expect_err("zero timeout");
    assert!(error
        .to_string()
        .contains("download timeout must be greater than zero"));
}

#[test]
fn update_progress_is_written_and_flushed_before_long_running_work() {
    #[derive(Default)]
    struct RecordingWriter {
        bytes: Vec<u8>,
        flushes: usize,
    }

    impl std::io::Write for RecordingWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    let mut writer = RecordingWriter::default();
    write_update_progress(&mut writer, "Applying update...\n").expect("progress write");
    assert_eq!(writer.bytes, b"Applying update...\n");
    assert_eq!(writer.flushes, 1);
}

#[tokio::test]
async fn update_is_unavailable_in_daemon_task_context() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("PATCHBAY_TASK_ID", "task-1");
    let cli = Cli::try_parse_from(["patchbay", "update"]).expect("update CLI");
    let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
        .await
        .expect_err("task context must be rejected");
    assert!(error
        .to_string()
        .contains("update is not available inside a daemon-managed task"));
}
