//! TEMPORARY S9-integration stand-in for lane E1b's codex_home port.
//! E1b replaces this file wholesale with the full codex_home.go port; only
//! the symbols execenv.rs's prepare path needs are declared here so the
//! workspace stays compilable while E1b is in flight.

/// `CodexHomeOptions` (codex_home.go:46–70) — field set used by prepare.
#[derive(Debug, Clone, Default)]
pub(crate) struct CodexHomeOptions {
    pub codex_version: String,
    pub goos: String,
    pub resume_session_id: String,
    pub is_local_directory: bool,
    pub session_store_key: String,
    pub codex_custom_args: Vec<String>,
}

/// `prepareCodexHomeWithOpts` — fail-open stand-in: creates an empty
/// per-task home. The real port seeds config.toml, sessions and plugin cache.
pub(crate) fn prepare_codex_home_with_opts(
    codex_home: &str,
    _opts: CodexHomeOptions,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(codex_home)
        .map_err(|e| anyhow::Error::new(e).context(format!("create {codex_home}")))
}

/// `codexSessionStoreKey` (codex_home.go:402).
pub(crate) fn codex_session_store_key(
    _profile: &str,
    _task: &super::execenv::TaskContextForEnv,
) -> String {
    String::new()
}
