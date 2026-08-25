use super::*;
use serde_json::Value;
use std::ffi::OsString;
use std::io::Cursor;

#[tokio::test]
async fn private_execenv_helper_dispatches_before_cli_parsing() {
    let missing = tempfile::tempdir()
        .expect("tempdir")
        .path()
        .join("missing-workdir");
    let input = serde_json::to_vec(&serde_json::json!({
        "action": "reuse",
        "reuse": {
            "WorkDir": missing,
            "Provider": "codex"
        }
    }))
    .expect("helper request");
    let mut output = Vec::new();

    let handled = run_private_helper(
        &[
            OsString::from("cordy"),
            OsString::from(cordy_daemon::execenv::isolation::PREPARATION_HELPER_ARG),
        ],
        Cursor::new(input),
        &mut output,
    )
    .await
    .expect("private helper");

    assert!(handled);
    let response: Value = serde_json::from_slice(&output).expect("helper response");
    assert!(response.get("environment").is_none());
    assert!(response.get("error").is_none());
}

#[tokio::test]
async fn private_execenv_helper_requires_the_exact_private_argv() {
    let mut output = Vec::new();
    let handled = run_private_helper(
        &[
            OsString::from("cordy"),
            OsString::from(cordy_daemon::execenv::isolation::PREPARATION_HELPER_ARG),
            OsString::from("unexpected"),
        ],
        Cursor::new(Vec::<u8>::new()),
        &mut output,
    )
    .await
    .expect("ordinary CLI path");

    assert!(!handled);
    assert!(output.is_empty());
}
