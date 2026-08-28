//! Typed local runtime probing for `patchbay daemon probe-runtimes`.
//!
//! The command intentionally goes through the daemon configuration loader,
//! rather than reimplementing PATH lookup in the CLI.  This keeps profile
//! selection, login-shell resolution, OpenClaw overrides, DSH validation, and
//! the complete built-in descriptor set on the same code path as daemon
//! startup.  The public report contains only provider names and counts; it
//! never exposes the resolved executable paths or credentials.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::agents_probe::probe_agent_clis;
use crate::config::{self, CliProfileConfig, Overrides};
use crate::types::AgentEntry;

/// Profile and CLI-config values needed by a local runtime probe.
///
/// This is deliberately separate from [`crate::assembly::DaemonProfileInput`]
/// because probing is unauthenticated and must not accept or carry a bearer
/// token.  Paths are consumed by the daemon config loader but are never
/// included in [`RuntimeProbeReport`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimeProbeOptions {
    /// Empty selects the default profile; otherwise this is the persisted
    /// profile name used by daemon config and identity resolution.
    pub profile: String,
    /// Per-runtime executable overrides from the selected CLI profile.
    pub profile_command_overrides: BTreeMap<String, String>,
    /// OpenClaw backend settings from the selected CLI profile.
    pub openclaw_binary_path: String,
    pub openclaw_state_dir: String,
    pub openclaw_cli_timeout: String,
}

/// Machine-local runtime probe result matching Go's JSON envelope.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct RuntimeProbeReport {
    pub probe_result: String,
    pub runtime_count: usize,
    pub provider_summary: BTreeMap<String, usize>,
}

/// Probe installed runtimes through the canonical daemon config and agent
/// discovery implementation.
pub fn probe_runtimes(options: RuntimeProbeOptions) -> anyhow::Result<RuntimeProbeReport> {
    probe_runtimes_with(options, &probe_agent_clis)
}

/// Test/embedding seam for the canonical probe.  Callers should normally use
/// [`probe_runtimes`]; this form lets unit tests inject a deterministic agent
/// discovery result without executing any process from the host PATH.
pub fn probe_runtimes_with(
    options: RuntimeProbeOptions,
    probe_agents: &dyn Fn() -> BTreeMap<String, AgentEntry>,
) -> anyhow::Result<RuntimeProbeReport> {
    let config = config::load_config(probe_overrides(options), probe_agents)?;
    Ok(report_from_agents(&config.agents))
}

fn probe_overrides(options: RuntimeProbeOptions) -> Overrides {
    Overrides {
        profile: options.profile,
        // Go's probe command explicitly uses AllowNoAgents=true: an empty
        // machine discovery is a successful report, not a startup failure.
        allow_no_agents: true,
        cli_profile_overrides: Some(CliProfileConfig {
            profile_command_overrides: options.profile_command_overrides,
            openclaw_binary_path: options.openclaw_binary_path,
            openclaw_state_dir: options.openclaw_state_dir,
            openclaw_cli_timeout: options.openclaw_cli_timeout,
        }),
        ..Overrides::default()
    }
}

fn report_from_agents(agents: &BTreeMap<String, AgentEntry>) -> RuntimeProbeReport {
    // Config discovery is already keyed by provider.  BTreeSet makes the
    // report robust if a future loader admits aliases while preserving the
    // deterministic provider ordering expected by JSON consumers.
    let providers = agents.keys().cloned().collect::<BTreeSet<_>>();
    let provider_summary = providers
        .into_iter()
        .map(|provider| (provider, 1usize))
        .collect::<BTreeMap<_, _>>();
    RuntimeProbeReport {
        probe_result: "success".to_string(),
        runtime_count: agents.len(),
        provider_summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str) -> AgentEntry {
        AgentEntry {
            path: path.to_string(),
            command: path.to_string(),
            model: String::new(),
        }
    }

    #[test]
    fn report_is_sorted_and_does_not_include_paths() {
        let agents = BTreeMap::from([
            ("zeta".to_string(), entry("/secret/zeta")),
            ("alpha".to_string(), entry("/secret/alpha")),
        ]);
        let report = report_from_agents(&agents);
        assert_eq!(report.probe_result, "success");
        assert_eq!(report.runtime_count, 2);
        assert_eq!(
            report.provider_summary,
            BTreeMap::from([("alpha".to_string(), 1), ("zeta".to_string(), 1),])
        );
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("/secret/"));
    }

    #[test]
    fn empty_agent_probe_is_allowed_and_preserves_profile_overrides() {
        let overrides = probe_overrides(RuntimeProbeOptions {
            profile: "staging".to_string(),
            profile_command_overrides: BTreeMap::from([(
                "profile-1".to_string(),
                "/opt/profile-1".to_string(),
            )]),
            ..RuntimeProbeOptions::default()
        });
        assert!(overrides.allow_no_agents);
        assert_eq!(overrides.profile, "staging");
        let cli = overrides.cli_profile_overrides.expect("CLI overrides");
        assert_eq!(cli.profile_command_overrides["profile-1"], "/opt/profile-1");
        let report = report_from_agents(&BTreeMap::new());
        assert_eq!(report.runtime_count, 0);
        assert!(report.provider_summary.is_empty());
    }
}
