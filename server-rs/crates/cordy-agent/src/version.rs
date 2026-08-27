//! Agent and daemon version policy.

use std::cmp::Ordering;
use std::sync::LazyLock;

use regex::Regex;

use crate::registry;

pub const MIN_QUICK_CREATE_CLI_VERSION: &str = "0.2.21";
pub const MIN_QUICK_CREATE_FIELDS_CLI_VERSION: &str = "0.4.3";
pub const MIN_LOCAL_WORKTREE_CLI_VERSION: &str = "0.4.24";
pub const MIN_HANDOFF_CLI_VERSION: &str = "0.3.28";

static VERSION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"v?(\d+)\.(\d+)\.(\d+)")
        .unwrap_or_else(|error| panic!("invalid version regex: {error}"))
});
static DEV_DESCRIBE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^v?\d+\.\d+\.\d+-\d+-g[0-9a-fA-F]+")
        .unwrap_or_else(|error| panic!("invalid git-describe regex: {error}"))
});

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Semver {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VersionError {
    #[error("CLI version is missing or invalid")]
    Missing,
    #[error("CLI version is below the required minimum")]
    TooOld,
}

pub fn parse_semver(value: &str) -> Result<Semver, VersionError> {
    let captures = VERSION_PATTERN
        .captures(value)
        .ok_or(VersionError::Missing)?;
    let parse = |index| {
        captures
            .get(index)
            .and_then(|capture| capture.as_str().parse::<u64>().ok())
            .ok_or(VersionError::Missing)
    };
    Ok(Semver {
        major: parse(1)?,
        minor: parse(2)?,
        patch: parse(3)?,
    })
}

pub fn check_minimum(
    value: &str,
    minimum: &str,
    allow_dev_describe: bool,
) -> Result<(), VersionError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(VersionError::Missing);
    }
    if allow_dev_describe && DEV_DESCRIBE_PATTERN.is_match(value) {
        return Ok(());
    }
    match parse_semver(value)?.cmp(&parse_semver(minimum)?) {
        Ordering::Less => Err(VersionError::TooOld),
        Ordering::Equal | Ordering::Greater => Ok(()),
    }
}

pub fn check_provider_minimum(provider: &str, version: &str) -> Result<(), VersionError> {
    let Some(minimum) = registry::provider(provider).and_then(|provider| provider.minimum_version)
    else {
        return Ok(());
    };
    check_minimum(version, minimum, false)
}

pub fn handoff_supported(version: &str) -> bool {
    check_minimum(version, MIN_HANDOFF_CLI_VERSION, true).is_ok()
}

/// Chooses the first line carrying a dotted semantic version. When a CLI has a
/// non-semver build id, its trimmed output remains observable.
pub fn extract_version_line(output: &str) -> String {
    output
        .lines()
        .map(str::trim)
        .find(|line| VERSION_PATTERN.is_match(line))
        .or_else(|| {
            let trimmed = output.trim();
            (!trimmed.is_empty()).then_some(trimmed)
        })
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prefixed_and_annotated_semver() {
        assert_eq!(
            parse_semver("codex-cli 0.118.0"),
            Ok(Semver {
                major: 0,
                minor: 118,
                patch: 0,
            })
        );
        assert_eq!(
            parse_semver("2.1.100 (Claude Code)"),
            Ok(Semver {
                major: 2,
                minor: 1,
                patch: 100,
            })
        );
        assert_eq!(parse_semver("invalid"), Err(VersionError::Missing));
    }

    #[test]
    fn provider_minimums_match_runtime_contract() {
        assert_eq!(
            check_provider_minimum("claude", "1.9.99"),
            Err(VersionError::TooOld)
        );
        assert!(check_provider_minimum("claude", "2.0.0").is_ok());
        assert_eq!(
            check_provider_minimum("codex", "0.99.0"),
            Err(VersionError::TooOld)
        );
        assert!(check_provider_minimum("qoderclicn", "unverified").is_ok());
        assert_eq!(
            check_provider_minimum("dim", "invalid"),
            Err(VersionError::Missing)
        );
    }

    #[test]
    fn daemon_gate_accepts_dev_describe_but_provider_gate_does_not() {
        assert!(check_minimum("v0.2.15-235-gdaf0e935", MIN_QUICK_CREATE_CLI_VERSION, true).is_ok());
        assert_eq!(
            check_minimum("v0.2.15", MIN_QUICK_CREATE_CLI_VERSION, true),
            Err(VersionError::TooOld)
        );
        assert!(!handoff_supported(""));
        assert!(handoff_supported("v0.3.28"));
    }

    #[test]
    fn version_line_skips_windows_codepage_noise() {
        assert_eq!(
            extract_version_line("Active code page: 65001\r\n0.42.0\r\n"),
            "0.42.0"
        );
        assert_eq!(extract_version_line(" some-build-id \n"), "some-build-id");
        assert_eq!(extract_version_line(""), "");
    }
}
