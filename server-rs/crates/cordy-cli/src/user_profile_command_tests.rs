use super::cli_test_helpers::*;
use super::*;
use clap::Parser;
use std::fs;
use std::io::Cursor;

#[tokio::test]
async fn user_profile_get_is_a_real_configured_api_command() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let config_dir = home.path().join(".cordy");
    fs::create_dir_all(&config_dir).expect("config dir");
    fs::write(
            config_dir.join("config.json"),
            r#"{"server_url":"http://127.0.0.1:1","token":"config-token","workspace_id":"config-workspace","future_field":true}"#,
        )
        .expect("config");
    let (server_url, server) = test_server().await;
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", format!("{server_url}/ws?discard=yes"));
    environment.set("CORDY_TOKEN", "token-from-env");
    environment.set("CORDY_WORKSPACE_ID", "workspace-from-env");
    let cli = Cli::try_parse_from(["cordy", "user", "profile", "get", "--output", "json"])
        .expect("parse CLI");

    let output = run(&cli, &environment).await.expect("run profile get");
    let json: Value = serde_json::from_str(&output.stdout).expect("JSON output");
    assert_eq!(json["profile_description"], "Maintainer");
    server.abort();
}

#[tokio::test]
async fn user_profile_update_patches_resolved_description() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let (server_url, captured, server) = patch_test_server().await;
    let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
    environment.set("CORDY_SERVER_URL", server_url);
    environment.set("CORDY_TOKEN", "token-from-env");
    let cli = Cli::try_parse_from([
        "cordy",
        "user",
        "profile",
        "update",
        "--description",
        r"Reviewer\nTypeScript",
        "--output",
        "json",
    ])
    .expect("parse CLI");
    let mut input = Cursor::new(Vec::<u8>::new());

    let output = run_with_input(&cli, &environment, &mut input)
        .await
        .expect("update profile");

    assert_eq!(
        captured
            .lock()
            .expect("captured body")
            .as_ref()
            .expect("body")["profile_description"],
        "Reviewer\nTypeScript"
    );
    let json: Value = serde_json::from_str(&output.stdout).expect("JSON output");
    assert_eq!(json["profile_description"], "Reviewer\nTypeScript");
    server.abort();
}

#[test]
fn profile_update_text_sources_match_go_semantics() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());

    let stdin_cli =
        Cli::try_parse_from(["cordy", "user", "profile", "update", "--description-stdin"])
            .expect("stdin CLI");
    let mut input = Cursor::new(b"first line\nsecond \\n literal\n".to_vec());
    assert_eq!(
        resolve_profile_description(update_args(&stdin_cli), &environment, &mut input)
            .expect("stdin description"),
        "first line\nsecond \\n literal"
    );

    fs::write(
        cwd.path().join("description.md"),
        "标题 / Заголовок\n\n中文段落\n",
    )
    .expect("description file");
    let file_cli = Cli::try_parse_from([
        "cordy",
        "user",
        "profile",
        "update",
        "--description-file",
        "description.md",
    ])
    .expect("file CLI");
    assert_eq!(
        resolve_profile_description(
            update_args(&file_cli),
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect("file description"),
        "标题 / Заголовок\n\n中文段落"
    );

    let empty_cli =
        Cli::try_parse_from(["cordy", "user", "profile", "update", "--description", ""])
            .expect("empty inline CLI");
    assert_eq!(
        resolve_profile_description(
            update_args(&empty_cli),
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect("empty inline clears"),
        ""
    );
}

#[test]
fn profile_update_rejects_ambiguous_or_empty_input() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let ambiguous = Cli::try_parse_from([
        "cordy",
        "user",
        "profile",
        "update",
        "--description",
        "inline",
        "--description-stdin",
    ])
    .expect("ambiguous CLI");
    assert!(resolve_profile_description(
        update_args(&ambiguous),
        &environment,
        &mut Cursor::new(b"stdin".to_vec())
    )
    .expect_err("ambiguous sources")
    .to_string()
    .contains("mutually exclusive"));

    let missing = Cli::try_parse_from(["cordy", "user", "profile", "update"]).expect("missing CLI");
    assert!(resolve_profile_description(
        update_args(&missing),
        &environment,
        &mut Cursor::new(Vec::<u8>::new())
    )
    .expect_err("missing source")
    .to_string()
    .contains("nothing to update"));

    let clear_with_input = Cli::try_parse_from([
        "cordy",
        "user",
        "profile",
        "update",
        "--clear",
        "--description",
        "inline",
    ])
    .expect("clear conflict CLI");
    assert!(resolve_profile_description(
        update_args(&clear_with_input),
        &environment,
        &mut Cursor::new(Vec::<u8>::new())
    )
    .expect_err("clear conflict")
    .to_string()
    .contains("--clear cannot be combined"));
}

#[test]
fn profile_update_file_input_fails_closed_outside_workdir() {
    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let outside = tempfile::tempdir().expect("outside dir");
    let external_path = outside.path().join("description.md");
    fs::write(&external_path, "external description").expect("external file");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let external_path = external_path.to_string_lossy().into_owned();
    let guarded = Cli::try_parse_from([
        "cordy",
        "user",
        "profile",
        "update",
        "--description-file",
        &external_path,
    ])
    .expect("guarded CLI");
    assert!(resolve_profile_description(
        update_args(&guarded),
        &environment,
        &mut Cursor::new(Vec::<u8>::new())
    )
    .expect_err("external file rejected")
    .to_string()
    .contains("--allow-external-file"));

    let allowed = Cli::try_parse_from([
        "cordy",
        "user",
        "profile",
        "update",
        "--description-file",
        &external_path,
        "--allow-external-file",
    ])
    .expect("allowed CLI");
    assert_eq!(
        resolve_profile_description(
            update_args(&allowed),
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect("external file allowed"),
        "external description"
    );
}

#[cfg(unix)]
#[test]
fn profile_update_rejects_workdir_symlink_that_escapes() {
    use std::os::unix::fs::symlink;

    let home = tempfile::tempdir().expect("temp home");
    let cwd = tempfile::tempdir().expect("temp cwd");
    let outside = tempfile::tempdir().expect("outside dir");
    let external_path = outside.path().join("description.md");
    fs::write(&external_path, "escaped description").expect("external file");
    symlink(&external_path, cwd.path().join("description.md")).expect("symlink");
    let environment = Environment::for_test(home.path().into(), cwd.path().into());
    let cli = Cli::try_parse_from([
        "cordy",
        "user",
        "profile",
        "update",
        "--description-file",
        "description.md",
    ])
    .expect("symlink CLI");

    assert!(resolve_profile_description(
        update_args(&cli),
        &environment,
        &mut Cursor::new(Vec::<u8>::new())
    )
    .expect_err("escaping symlink rejected")
    .to_string()
    .contains("--allow-external-file"));
}

#[test]
fn table_output_matches_go_vertical_table_contract() {
    let profile = serde_json::json!({"id":"user-1","name":"Ada","email":"ada@example.com"});
    assert_eq!(
            format_user_profile_table(&profile),
            "ID                   user-1\nNAME                 Ada\nEMAIL                ada@example.com\nPROFILE DESCRIPTION  (not set)\n"
        );
}
