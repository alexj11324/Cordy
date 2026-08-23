//! Port of execenv/codex_sandbox.go.
//!
//! Symbol map:
//! - CodexDarwinNetworkAccessFixedVersion → CODEX_DARWIN_NETWORK_ACCESS_FIXED_VERSION
//! - codexSandboxPolicy        → CodexSandboxPolicy
//! - resolveGOOS               → resolve_goos
//! - codexSandboxPolicyFor     → codex_sandbox_policy_for
//! - windowsSandboxConfig      → WindowsSandboxConfig
//! - codexSandboxPolicyForConfig → codex_sandbox_policy_for_config
//! - codexSandboxPolicyForWindows → codex_sandbox_policy_for_windows
//! - windowsSandboxFromConfig  → windows_sandbox_from_config
//! - codexWindowsSandboxOverrideRe → codex_windows_sandbox_override_re
//! - windowsSandboxFromCustomArgs → windows_sandbox_from_custom_args
//! - classifyWindowsSandboxValue → classify_windows_sandbox_value
//! - resolveWindowsSandbox     → resolve_windows_sandbox
//! - codexDarwinNetworkAccessFixed → codex_darwin_network_access_fixed
//! - codexUpgradeHint / codexLinuxIsolationHint → codex_upgrade_hint / codex_linux_isolation_hint
//! - cordyManagedBegin/EndMarker → CORDY_MANAGED_BEGIN_MARKER / CORDY_MANAGED_END_MARKER
//! - renderCordyManagedBlock   → render_cordy_managed_block
//! - managedBlockRe            → managed_block_re
//! - upsertCordyManagedBlock   → upsert_cordy_managed_block
//! - stripLegacySandboxDirectives → strip_legacy_sandbox_directives
//! - ensureCodexSandboxConfig  → ensure_codex_sandbox_config
//! - codexSemver / parseCodexSemver / lessThan → CodexSemver / parse_codex_semver / is_less_than
//!
//! Deviations:
//! - slog logger parameter dropped; tracing macros used directly.
//! - Go's %q formatting becomes Rust's {:?}; identical bytes for the plain
//!   ASCII sandbox_mode values this file emits.

use anyhow::Context;
use regex::Regex;

/// Earliest Codex CLI version in which `network_access = true` is honored
/// under Seatbelt on macOS. Bump when the upstream fix ships. Empty means "no
/// known fixed release yet — always treat macOS Codex as broken for network
/// access".
pub const CODEX_DARWIN_NETWORK_ACCESS_FIXED_VERSION: &str = "";

/// Describes how the per-task Codex config.toml should configure the sandbox.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodexSandboxPolicy {
    /// Value written as `sandbox_mode = "..."`.
    pub mode: String,
    /// Controls `[sandbox_workspace_write] network_access`. Only meaningful
    /// when mode is "workspace-write".
    pub network_access: bool,
    /// Short human-readable label used in warn-level logs.
    pub reason: String,
    /// Optional actionable remediation surfaced in warn-level logs when mode
    /// is danger-full-access. Empty when there is no generic action to surface.
    pub hint: String,
}

/// Returns goos, or the host platform when goos is empty. Callers pass an
/// explicit goos in tests; production leaves it empty to use the host.
pub(crate) fn resolve_goos(goos: &str) -> String {
    if goos.is_empty() {
        return std::env::consts::OS.to_string();
    }
    goos.to_string()
}

/// Picks the default policy for the given platform and detected Codex CLI
/// version. It is the platform baseline; per-task user config can refine it
/// (see codex_sandbox_policy_for_config).
///
///   - Linux (and any other non-darwin, non-windows platform):
///     danger-full-access on the daemon user's real HOME (MUL-5578 / #6218).
///   - Windows: danger-full-access as a deliberate compatibility choice
///     (MUL-4957); a user who opted into windows.sandbox keeps workspace-write
///     via codex_sandbox_policy_for_windows instead.
///   - darwin at or above CODEX_DARWIN_NETWORK_ACCESS_FIXED_VERSION:
///     workspace-write with network access (upstream bug fixed).
///   - darwin otherwise (including unknown version): danger-full-access so the
///     Cordy CLI can reach the API (openai/codex#10390).
pub(crate) fn codex_sandbox_policy_for(goos: &str, detected_version: &str) -> CodexSandboxPolicy {
    let goos = resolve_goos(goos);
    if goos == "windows" {
        return CodexSandboxPolicy {
            mode: "danger-full-access".to_string(),
            reason: "codex on windows: compatibility fallback; no native windows.sandbox configured, so workspace-write cannot be enforced (MUL-4957)".to_string(),
            ..Default::default()
        };
    }
    if goos != "darwin" {
        return CodexSandboxPolicy {
            mode: "danger-full-access".to_string(),
            reason: format!(
                "codex on {goos}: tasks run with the daemon user's real HOME and full filesystem access; isolation comes from the boundary the daemon runs inside (MUL-5578)"
            ),
            hint: codex_linux_isolation_hint(),
            ..Default::default()
        };
    }
    if codex_darwin_network_access_fixed(detected_version) {
        return CodexSandboxPolicy {
            mode: "workspace-write".to_string(),
            network_access: true,
            reason: "codex version includes macOS network_access fix".to_string(),
            ..Default::default()
        };
    }
    let mut reason =
        "codex on macOS: seatbelt ignores sandbox_workspace_write.network_access (openai/codex#10390)"
            .to_string();
    if detected_version.is_empty() {
        reason.push_str(" — version unknown, assuming broken");
    }
    CodexSandboxPolicy {
        mode: "danger-full-access".to_string(),
        network_access: false,
        reason,
        hint: codex_upgrade_hint(),
    }
}

/// Tri-state of a native Codex Windows sandbox selection. Three-valued (not a
/// bool) so an undecidable config fails closed — the daemon never loosens to
/// danger-full-access when it cannot confirm the user's intent (MUL-4957).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowsSandboxConfig {
    /// Confidently no native sandbox selected anywhere, so the
    /// danger-full-access compatibility fallback is safe to apply.
    Absent,
    /// A valid windows.sandbox = "unelevated"|"elevated" is selected, so keep
    /// workspace-write and let Codex enforce isolation.
    Native,
    /// The config could not be read/parsed, or holds a windows.sandbox value
    /// Codex does not accept. Must NOT loosen — fail closed.
    Undecidable,
}

/// Returns the platform default from codex_sandbox_policy_for for
/// linux/darwin. On Windows it applies the resolved native-sandbox state: a
/// user who opted into windows.sandbox keeps workspace-write, an undecidable
/// config fails closed to workspace-write, and only a confidently absent
/// sandbox gets the danger-full-access compatibility fallback (MUL-4957).
///
/// This is intentionally the branch point for the eventual native-sandbox
/// rollout: flipping the Windows default later means writing windows.sandbox
/// ourselves and defaulting win_state to native, not restructuring callers.
pub(crate) fn codex_sandbox_policy_for_config(
    goos: &str,
    detected_version: &str,
    win_state: WindowsSandboxConfig,
) -> CodexSandboxPolicy {
    let goos = resolve_goos(goos);
    if goos == "windows" {
        return codex_sandbox_policy_for_windows(win_state);
    }
    codex_sandbox_policy_for(&goos, detected_version)
}

/// Maps a resolved native-sandbox state to a policy. Native and Undecidable
/// both keep workspace-write (Undecidable is the fail-closed case — it never
/// loosens on doubt); only a confidently absent native sandbox gets the
/// danger-full-access compatibility fallback.
fn codex_sandbox_policy_for_windows(state: WindowsSandboxConfig) -> CodexSandboxPolicy {
    match state {
        WindowsSandboxConfig::Native => CodexSandboxPolicy {
            mode: "workspace-write".to_string(),
            network_access: true,
            reason: "codex on windows: native windows.sandbox configured; keeping workspace-write so Codex enforces task isolation".to_string(),
            ..Default::default()
        },
        WindowsSandboxConfig::Undecidable => CodexSandboxPolicy {
            mode: "workspace-write".to_string(),
            network_access: true,
            reason: "codex on windows: windows.sandbox config undecidable (unreadable/unparseable/invalid); failing closed to workspace-write rather than loosening (MUL-4957)".to_string(),
            ..Default::default()
        },
        WindowsSandboxConfig::Absent => CodexSandboxPolicy {
            mode: "danger-full-access".to_string(),
            reason: "codex on windows: compatibility fallback; no native windows.sandbox configured (MUL-4957)".to_string(),
            ..Default::default()
        },
    }
}

/// Classifies the windows.sandbox selection in a config.toml body. Codex
/// accepts only the exact-lowercase variants "unelevated"/"elevated" and
/// refuses to load a config with any other value, so anything else present is
/// treated as undecidable (fail closed) rather than a safe "absent".
/// Unparseable TOML is likewise undecidable — Codex would reject the same
/// file. An absent windows.sandbox key is a genuine "absent".
pub(crate) fn windows_sandbox_from_config(config: &str) -> WindowsSandboxConfig {
    let probe: toml::Value = match config.parse() {
        Ok(v) => v,
        Err(_) => return WindowsSandboxConfig::Undecidable,
    };
    let sandbox = probe
        .get("windows")
        .and_then(|w| w.get("sandbox"))
        .and_then(|s| s.as_str())
        .unwrap_or("");
    classify_windows_sandbox_value(sandbox)
}

fn codex_windows_sandbox_override_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*windows\s*\.\s*sandbox\s*=").expect("static regex"))
}

/// Classifies a native Windows sandbox selection passed via Codex `-c
/// windows.sandbox=...` / `--config windows.sandbox=...` args. These never land
/// in config.toml (they stay in argv and are applied on top of it), so
/// config-only detection would miss them — the MUL-4957 review's second
/// must-fix. Mirrors the override-parsing shape in server/pkg/agent's
/// buildCodexArgs: inline (`-c=windows.sandbox=x`) and two-token (`-c
/// windows.sandbox=x`) forms, last occurrence winning (Codex is last-wins).
pub(crate) fn windows_sandbox_from_custom_args(args: &[String]) -> WindowsSandboxConfig {
    let mut state = WindowsSandboxConfig::Absent;
    let mut i = 0usize;
    while i < args.len() {
        let arg = &args[i];
        let mut flag = arg.as_str();
        let mut value = "";
        let mut has_inline_value = false;
        if let Some(idx) = arg.find('=') {
            if idx > 0 {
                flag = &arg[..idx];
                value = &arg[idx + 1..];
                has_inline_value = true;
            }
        }
        if flag != "-c" && flag != "--config" {
            i += 1;
            continue;
        }
        if !has_inline_value {
            i += 1;
            if i >= args.len() {
                break;
            }
            value = &args[i];
        }
        if !codex_windows_sandbox_override_re().is_match(value) {
            i += 1;
            continue;
        }
        // A windows.sandbox override token: take the part after its first `=`.
        if let Some(eq) = value.find('=') {
            state = classify_windows_sandbox_value(&value[eq + 1..]);
        }
        i += 1;
    }
    state
}

/// Maps a raw windows.sandbox value (from config.toml or a `-c` arg, possibly
/// surrounded by whitespace/quotes) to a tri-state. Only the exact-lowercase
/// variants Codex accepts count as native; a present but unaccepted value is
/// undecidable (Codex would refuse the config); an empty value is absent.
pub(crate) fn classify_windows_sandbox_value(raw: &str) -> WindowsSandboxConfig {
    let v = raw.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    match v {
        "" => WindowsSandboxConfig::Absent,
        "unelevated" | "elevated" => WindowsSandboxConfig::Native,
        _ => WindowsSandboxConfig::Undecidable,
    }
}

/// Folds per-layer states into one. Undecidable wins over everything (any
/// broken/ambiguous layer means fail closed), then Native over Absent (an
/// opt-in in any layer keeps isolation).
pub(crate) fn resolve_windows_sandbox(states: &[WindowsSandboxConfig]) -> WindowsSandboxConfig {
    let mut result = WindowsSandboxConfig::Absent;
    for s in states {
        if *s == WindowsSandboxConfig::Undecidable {
            return WindowsSandboxConfig::Undecidable;
        }
        if *s == WindowsSandboxConfig::Native {
            result = WindowsSandboxConfig::Native;
        }
    }
    result
}

/// Returns true if the given detected version is known to honor `network_access
/// = true` under Seatbelt on macOS.
pub(crate) fn codex_darwin_network_access_fixed(detected_version: &str) -> bool {
    if CODEX_DARWIN_NETWORK_ACCESS_FIXED_VERSION.is_empty() || detected_version.is_empty() {
        return false;
    }
    let Ok(fixed) = parse_codex_semver(CODEX_DARWIN_NETWORK_ACCESS_FIXED_VERSION) else {
        return false;
    };
    let Ok(got) = parse_codex_semver(detected_version) else {
        return false;
    };
    !got.is_less_than(fixed)
}

/// Short, actionable hint for users running a Codex version that suffers from
/// the macOS network_access bug.
fn codex_upgrade_hint() -> String {
    "upgrade Codex CLI (e.g. `brew upgrade codex` or `npm i -g @openai/codex`) once a release including openai/codex#10390 is available to restore workspace-write + network_access".to_string()
}

/// Actionable remediation for the Linux full-access default: unlike the macOS
/// bug, there is no version to upgrade to — the containment has to come from
/// the boundary the daemon runs inside.
fn codex_linux_isolation_hint() -> String {
    "run the daemon inside a VM, container, or dedicated Unix user — tasks can read and write everything that user can".to_string()
}

/// Delimit the block the daemon writes into the per-task config.toml.
/// Everything between the markers is owned by the daemon and will be rewritten
/// idempotently; anything outside the markers is preserved as-is.
const CORDY_MANAGED_BEGIN_MARKER: &str =
    "# BEGIN cordy-managed (do not edit; regenerated by daemon)";
const CORDY_MANAGED_END_MARKER: &str = "# END cordy-managed";

/// Produces the managed block for the given policy.
///
/// The block contains only top-level key=value assignments — no `[table]`
/// headers — and uses TOML dotted-key syntax for nested values. This matters
/// because the block is inserted into a user-owned config.toml: opening a
/// `[sandbox_workspace_write]` header would silently reparent any user content
/// below it, and appending after a file that ends inside some other table
/// would parse bare keys as children of that table. Keeping the block as pure
/// top-level dotted-key assignments, placed at the top of the file, avoids
/// both traps.
fn render_cordy_managed_block(policy: &CodexSandboxPolicy) -> String {
    let mut b = String::new();
    b.push_str(CORDY_MANAGED_BEGIN_MARKER);
    b.push('\n');
    b.push_str(&format!("sandbox_mode = {:?}\n", policy.mode));
    if policy.mode == "workspace-write" {
        b.push_str(&format!(
            "sandbox_workspace_write.network_access = {}\n",
            policy.network_access
        ));
    }
    b.push_str(CORDY_MANAGED_END_MARKER);
    b.push('\n');
    b
}

/// Captures the daemon-owned block (including the surrounding markers and any
/// trailing blank lines) so it can be replaced idempotently. `\n*` rather than
/// `\n?` so reruns don't accumulate blank lines when the block coexists with
/// another managed block (e.g. multi-agent) in the file.
fn managed_block_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"(?ms)^{}.*?^{}\n*",
            regex::escape(CORDY_MANAGED_BEGIN_MARKER),
            regex::escape(CORDY_MANAGED_END_MARKER)
        ))
        .expect("static regex")
    })
}

/// Returns the config content with the cordy-managed block placed at the very
/// top of the file. Any previously written managed block is removed in place;
/// user content outside the markers is preserved.
///
/// The block is always hoisted to the top (rather than replaced in place or
/// appended to EOF) so that its top-level keys are parsed at the TOML root,
/// regardless of whether the user's config ends inside a table like
/// `[permissions.cordy]`. Combined with the dotted-key form used by
/// render_cordy_managed_block, this means the managed block neither leaks into
/// nor inherits from any surrounding table scope.
fn upsert_cordy_managed_block(content: &str, policy: &CodexSandboxPolicy) -> String {
    // Drop any previously written managed block (wherever it sits).
    let content = managed_block_re().replace_all(content, "").into_owned();
    let block = render_cordy_managed_block(policy);
    // Trim leading blank lines left behind by the removal so we don't grow the
    // file on every idempotent rewrite.
    let content = content.trim_start_matches('\n');
    if content.is_empty() {
        return block;
    }
    format!("{block}\n{content}")
}

/// Removes top-level `sandbox_mode = ...` lines and any
/// `[sandbox_workspace_write]` section that would otherwise conflict with the
/// managed block. Only top-level entries are stripped; anything under an
/// unrelated section header (like `[permissions.foo]`) is preserved untouched.
fn strip_legacy_sandbox_directives(content: &str) -> String {
    let mut out: Vec<&str> = Vec::with_capacity(content.lines().count());
    let mut in_legacy_workspace_write = false;
    for line in content.split('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            // Entering a new section. Exit legacy-tracking if we were in one.
            in_legacy_workspace_write = trimmed == "[sandbox_workspace_write]";
            if in_legacy_workspace_write {
                continue;
            }
            out.push(line);
            continue;
        }
        if in_legacy_workspace_write {
            // Drop the legacy section body until the next section.
            continue;
        }
        if trimmed.starts_with("sandbox_mode") {
            // Drop legacy top-level sandbox_mode declarations.
            continue;
        }
        out.push(line);
    }
    out.join("\n")
}

/// Writes the cordy-managed sandbox block into the given config.toml according
/// to the policy. Idempotent: running it twice produces the same file
/// contents. The file is created if it doesn't exist.
///
/// Logs (at warn level) whenever the resolved mode is danger-full-access — the
/// Linux default, the macOS seatbelt fallback, and the Windows
/// no-native-sandbox fallback alike — so every unsandboxed task is visible in
/// daemon logs.
pub(crate) fn ensure_codex_sandbox_config(
    config_path: &str,
    policy: &CodexSandboxPolicy,
    detected_version: &str,
) -> anyhow::Result<()> {
    let data = match std::fs::read_to_string(config_path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(anyhow::Error::new(e).context("read config.toml")),
    };

    // Drop inline sandbox_mode / [sandbox_workspace_write] from older daemon
    // versions so they don't collide with the managed block.
    let mut existing = data.clone();
    if !existing.is_empty() && !managed_block_re().is_match(&existing) {
        existing = strip_legacy_sandbox_directives(&existing);
    }

    let updated = upsert_cordy_managed_block(&existing, policy);
    if updated == data {
        return Ok(());
    }

    if policy.mode == "danger-full-access" {
        let version = if detected_version.is_empty() {
            "unknown"
        } else {
            detected_version
        };
        tracing::warn!(
            reason = %policy.reason,
            codex_version = %version,
            config_path = %config_path,
            hint = %policy.hint,
            "codex sandbox: running unsandboxed with danger-full-access"
        );
    }

    std::fs::write(config_path, updated).context("write config.toml")?;
    Ok(())
}

// --- small semver helper, scoped to this module to avoid a dependency on the
// agent package's parser (mirrors the Go comment about avoiding an import
// cycle).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CodexSemver {
    major: i64,
    minor: i64,
    patch: i64,
}

fn parse_codex_semver(raw: &str) -> Result<CodexSemver, anyhow::Error> {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"v?(\d+)\.(\d+)\.(\d+)").expect("static regex"));
    let caps = re
        .captures(raw)
        .ok_or_else(|| anyhow::anyhow!("cannot parse version {raw:?}"))?;
    Ok(CodexSemver {
        major: caps[1].parse().unwrap_or(0),
        minor: caps[2].parse().unwrap_or(0),
        patch: caps[3].parse().unwrap_or(0),
    })
}

impl CodexSemver {
    fn is_less_than(self, o: CodexSemver) -> bool {
        if self.major != o.major {
            return self.major < o.major;
        }
        if self.minor != o.minor {
            return self.minor < o.minor;
        }
        self.patch < o.patch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of TestCodexSandboxPolicyFor platform table.
    #[test]
    fn test_codex_sandbox_policy_for_platform_table() {
        // Linux: deliberate full access with isolation hint (MUL-5578).
        let p = codex_sandbox_policy_for("linux", "0.121.0");
        assert_eq!(p.mode, "danger-full-access");
        assert!(p.reason.contains("MUL-5578"));
        assert_eq!(p.hint, codex_linux_isolation_hint());

        // Windows baseline: compatibility fallback (MUL-4957).
        let p = codex_sandbox_policy_for("windows", "");
        assert_eq!(p.mode, "danger-full-access");
        assert!(p.reason.contains("MUL-4957"));

        // darwin with no fixed release known: always broken.
        let p = codex_sandbox_policy_for("darwin", "9.9.9");
        assert_eq!(p.mode, "danger-full-access");
        assert!(!p.reason.contains("version unknown"));

        // darwin with unknown version: broken + explicit note.
        let p = codex_sandbox_policy_for("darwin", "");
        assert_eq!(p.mode, "danger-full-access");
        assert!(p.reason.contains("version unknown"));
        assert_eq!(p.hint, codex_upgrade_hint());

        // Empty goos resolves to the host platform (whatever it is) without
        // panicking.
        let _ = codex_sandbox_policy_for("", "1.0.0");
    }

    // Port of TestClassifyWindowsSandboxValue: only exact-lowercase variants
    // count as native; quoting/whitespace tolerated; junk is undecidable.
    #[test]
    fn test_classify_windows_sandbox_value() {
        assert_eq!(
            classify_windows_sandbox_value(""),
            WindowsSandboxConfig::Absent
        );
        assert_eq!(
            classify_windows_sandbox_value("  \"unelevated\" "),
            WindowsSandboxConfig::Native
        );
        assert_eq!(
            classify_windows_sandbox_value("'elevated'"),
            WindowsSandboxConfig::Native
        );
        assert_eq!(
            classify_windows_sandbox_value("Unelevated"),
            WindowsSandboxConfig::Undecidable
        );
        assert_eq!(
            classify_windows_sandbox_value("paranoid"),
            WindowsSandboxConfig::Undecidable
        );
    }

    // Port of TestWindowsSandboxFromCustomArgs: inline and two-token forms,
    // last occurrence winning.
    #[test]
    fn test_windows_sandbox_from_custom_args() {
        assert_eq!(
            windows_sandbox_from_custom_args(&[]),
            WindowsSandboxConfig::Absent
        );
        let mk = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        // Inline form.
        assert_eq!(
            windows_sandbox_from_custom_args(&mk(&["-c=windows.sandbox=\"unelevated\""])),
            WindowsSandboxConfig::Native
        );
        // Two-token form.
        assert_eq!(
            windows_sandbox_from_custom_args(&mk(&["--config", "windows.sandbox=elevated"])),
            WindowsSandboxConfig::Native
        );
        // Unrelated -c overrides ignored.
        assert_eq!(
            windows_sandbox_from_custom_args(&mk(&["-c", "model=gpt-5"])),
            WindowsSandboxConfig::Absent
        );
        // Invalid value propagates as undecidable (fail closed).
        assert_eq!(
            windows_sandbox_from_custom_args(&mk(&["-c", "windows.sandbox=bogus"])),
            WindowsSandboxConfig::Undecidable
        );
        // Last occurrence wins.
        assert_eq!(
            windows_sandbox_from_custom_args(&mk(&[
                "-c",
                "windows.sandbox=elevated",
                "-c",
                "windows.sandbox="
            ])),
            WindowsSandboxConfig::Absent
        );
        // Trailing flag without a value is skipped safely.
        assert_eq!(
            windows_sandbox_from_custom_args(&mk(&["-c"])),
            WindowsSandboxConfig::Absent
        );
    }

    // Port of TestResolveWindowsSandbox precedence: Undecidable > Native >
    // Absent.
    #[test]
    fn test_resolve_windows_sandbox_precedence() {
        use WindowsSandboxConfig::*;
        assert_eq!(resolve_windows_sandbox(&[]), Absent);
        assert_eq!(resolve_windows_sandbox(&[Absent]), Absent);
        assert_eq!(resolve_windows_sandbox(&[Absent, Native]), Native);
        assert_eq!(resolve_windows_sandbox(&[Native, Absent]), Native);
        assert_eq!(resolve_windows_sandbox(&[Native, Undecidable]), Undecidable);
        assert_eq!(resolve_windows_sandbox(&[Undecidable, Native]), Undecidable);
    }

    // Port of TestWindowsSandboxFromConfig: unparseable TOML undecidable,
    // missing key absent, valid values classified.
    #[test]
    fn test_windows_sandbox_from_config() {
        assert_eq!(
            windows_sandbox_from_config("[windows]\nsandbox = \"elevated\"\n"),
            WindowsSandboxConfig::Native
        );
        assert_eq!(
            windows_sandbox_from_config("[other]\nkey = 1\n"),
            WindowsSandboxConfig::Absent
        );
        assert_eq!(
            windows_sandbox_from_config("not [valid toml"),
            WindowsSandboxConfig::Undecidable
        );
    }

    // Port of TestRenderAndUpsertCordyManagedBlock: idempotent rewrite,
    // hoisted to the top, user content preserved byte-for-byte.
    #[test]
    fn test_upsert_cordy_managed_block_idempotent() {
        let policy = CodexSandboxPolicy {
            mode: "workspace-write".to_string(),
            network_access: true,
            ..Default::default()
        };
        let once = upsert_cordy_managed_block("", &policy);
        assert!(once.starts_with(CORDY_MANAGED_BEGIN_MARKER));
        assert!(once.contains("sandbox_mode = \"workspace-write\""));
        assert!(once.contains("sandbox_workspace_write.network_access = true"));

        // Idempotent: rewriting the output yields the same bytes.
        let twice = upsert_cordy_managed_block(&once, &policy);
        assert_eq!(once, twice);

        // User content below a trailing table survives and stays after the
        // block.
        let user = "[profiles.foo]\nmodel = \"gpt\"\n";
        let merged = upsert_cordy_managed_block(user, &policy);
        assert!(merged.starts_with(CORDY_MANAGED_BEGIN_MARKER));
        assert!(merged.ends_with(user));

        // An old managed block sitting mid-file is removed wherever it sits.
        let stale = format!("keep-a\n{once}\nkeep-b\n");
        let cleaned = upsert_cordy_managed_block(&stale, &policy);
        assert!(cleaned.starts_with(CORDY_MANAGED_BEGIN_MARKER));
        assert!(cleaned.contains("keep-a"));
        assert!(cleaned.contains("keep-b"));
        assert_eq!(cleaned.matches(CORDY_MANAGED_BEGIN_MARKER).count(), 1);

        // danger-full-access omits the network_access line entirely.
        let full = CodexSandboxPolicy {
            mode: "danger-full-access".to_string(),
            ..Default::default()
        };
        let rendered = render_cordy_managed_block(&full);
        assert!(rendered.contains("sandbox_mode = \"danger-full-access\""));
        assert!(!rendered.contains("network_access"));
    }

    // Port of TestStripLegacySandboxDirectives: only top-level entries and the
    // exact legacy section are stripped.
    #[test]
    fn test_strip_legacy_sandbox_directives() {
        let input = "top = 1\nsandbox_mode = \"old\"\n[sandbox_workspace_write]\nnetwork_access = true\n[permissions.cordy]\nsandbox_mode = \"kept\"\n";
        let got = strip_legacy_sandbox_directives(input);
        assert!(got.contains("top = 1"));
        assert!(!got.lines().any(|l| l.trim() == "[sandbox_workspace_write]"));
        assert!(!got.contains("network_access = true"));
        // Go strips any line prefixed sandbox_mode (section-agnostic prefix
        // check), so even the one under [permissions.cordy] goes. The section
        // header itself survives.
        assert!(got.contains("[permissions.cordy]"));
        assert!(!got.contains("\"kept\""));
    }

    // Port of TestEnsureCodexSandboxConfigIdempotent (filesystem half).
    #[test]
    fn test_ensure_codex_sandbox_config_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("config.toml")
            .to_string_lossy()
            .into_owned();

        let policy = CodexSandboxPolicy {
            mode: "danger-full-access".to_string(),
            reason: "test".to_string(),
            ..Default::default()
        };
        ensure_codex_sandbox_config(&path, &policy, "0.121.0").unwrap();
        let first = std::fs::read_to_string(&path).unwrap();

        // Second run produces identical bytes.
        ensure_codex_sandbox_config(&path, &policy, "0.121.0").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);

        // Legacy inline directives from older daemons are migrated away.
        std::fs::write(&path, "sandbox_mode = \"legacy\"\nuser_key = 1\n").unwrap();
        ensure_codex_sandbox_config(&path, &policy, "").unwrap();
        let migrated = std::fs::read_to_string(&path).unwrap();
        assert!(migrated.starts_with(CORDY_MANAGED_BEGIN_MARKER));
        assert!(migrated.contains("user_key = 1"));
        assert!(!migrated.contains("\"legacy\""));
    }

    // Port of TestParseCodexSemver.
    #[test]
    fn test_parse_codex_semver() {
        assert_eq!(
            parse_codex_semver("v1.2.3").unwrap(),
            CodexSemver {
                major: 1,
                minor: 2,
                patch: 3
            }
        );
        assert_eq!(
            parse_codex_semver("codex-cli 0.121.0 (abc)").unwrap(),
            CodexSemver {
                major: 0,
                minor: 121,
                patch: 0
            }
        );
        assert!(parse_codex_semver("not-a-version").is_err());

        let low = parse_codex_semver("0.120.9").unwrap();
        let high = parse_codex_semver("0.121.0").unwrap();
        assert!(low.is_less_than(high));
        assert!(!high.is_less_than(low));
        assert!(!high.is_less_than(high));
    }

    // Port of TestCodexDarwinNetworkAccessFixed: constant empty → always
    // false until the upstream fix ships.
    #[test]
    fn test_codex_darwin_network_access_fixed_disabled() {
        assert!(!codex_darwin_network_access_fixed(""));
        assert!(!codex_darwin_network_access_fixed("999.0.0"));
    }
}
