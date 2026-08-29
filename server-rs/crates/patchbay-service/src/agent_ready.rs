//! Agent readiness verdicts — full port of `service/agent_ready.go`.
//!
//! The distinction that matters is not "ready or not" but whether WAITING is
//! a plan: a sleeping laptop comes back on its own (waitable), an agent bound
//! to nothing or to a machine whose CLI cannot run never picks work up until
//! a human acts (blocked).

use patchbay_db::models::{Agent, AgentRuntime};
use patchbay_db::queries::runtime::get_agent_runtime;

use crate::dispatch_reason::ReasonCode;

/// What a readiness check concluded, in the vocabulary callers branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentAvailability {
    /// The agent can take work now.
    Available,
    /// Not runnable right now, but nothing is broken — the machine is offline
    /// and queued work runs when it returns.
    Waitable,
    /// Nothing will claim this agent's work until someone intervenes; refuse
    /// the trigger and say why.
    Blocked,
}

/// The npm package owning a broken entry point and the command that
/// reinstalls it — mirrors the daemon's agent.ExecFormatRepair over the wire.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct RuntimeRepair {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub package: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    /// Interpreter Command is written for ("bash", "powershell"), rendered as
    /// the code fence language. Empty from a daemon too old to report it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shell: String,
}

/// A readiness decision plus everything a caller needs to act on it.
#[derive(Debug, Clone)]
pub struct AgentVerdict {
    pub availability: AgentAvailability,
    /// Dispatch code for a non-available verdict; meaningless otherwise.
    pub reason: ReasonCode,
    /// Daemon-reported fix for an unusable runtime. Absent for every other
    /// verdict and for daemons too old to report one.
    pub repair: Option<RuntimeRepair>,
    /// The daemon's own description — logs and blocked-trigger records only,
    /// never parsed.
    pub detail: String,
}

impl AgentVerdict {
    /// Whether the agent can take work right now.
    pub fn ready(&self) -> bool {
        self.availability == AgentAvailability::Available
    }

    /// Whether waiting is futile and the caller must refuse the trigger.
    pub fn blocked(&self) -> bool {
        self.availability == AgentAvailability::Blocked
    }
}

/// The daemon's structured explanation, stored on the runtime row's metadata
/// by the deregister handler.
#[derive(Debug, Clone, serde::Deserialize)]
struct RuntimeOfflineReason {
    code: String,
    detail: String,
    repair: Option<RuntimeRepair>,
}

/// The daemon's code for "the OS refuses to execute this agent CLI"
/// (daemon.RuntimeOfflineCodeNotExecutable). Compared as a string because the
/// server must not import the daemon package.
const RUNTIME_OFFLINE_CODE_NOT_EXECUTABLE: &str = "not_executable";

/// Reports whether an agent can accept new work right now, and what the
/// caller should do when it cannot.
///
/// The error case is DB lookup failure only. Callers that treat a transient
/// error as "do not skip" (the autopilot admission gate) swallow it; callers
/// needing a hard yes/no (team-leader pre-enqueue checks) fail closed.
pub async fn agent_readiness<'e, E>(executor: E, agent: &Agent) -> anyhow::Result<AgentVerdict>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    if agent.archived_at.is_some() {
        return Ok(AgentVerdict {
            availability: AgentAvailability::Blocked,
            reason: ReasonCode::TargetUnavailable,
            repair: None,
            detail: "agent is archived".to_string(),
        });
    }
    let Some(runtime_id) = agent.runtime_id else {
        return Ok(AgentVerdict {
            availability: AgentAvailability::Blocked,
            reason: ReasonCode::AgentRuntimeRequired,
            repair: None,
            detail: "agent has no runtime bound".to_string(),
        });
    };
    let rt = get_agent_runtime(executor, runtime_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("agent readiness: runtime not found"))?;
    Ok(runtime_verdict(&rt))
}

/// The half of the decision that depends only on the runtime row, split out
/// so every branch is testable without a database.
pub fn runtime_verdict(rt: &AgentRuntime) -> AgentVerdict {
    if rt.status == "online" {
        return AgentVerdict {
            availability: AgentAvailability::Available,
            reason: ReasonCode::InternalError,
            repair: None,
            detail: String::new(),
        };
    }
    // Offline with a reason the daemon says a human must repair: refuse
    // rather than queue, and carry the repair so the caller can show it.
    if let Some(reason) = parse_runtime_offline_reason(&rt.metadata) {
        if reason.code == RUNTIME_OFFLINE_CODE_NOT_EXECUTABLE {
            return AgentVerdict {
                availability: AgentAvailability::Blocked,
                reason: ReasonCode::RuntimeUnusable,
                repair: reason.repair,
                detail: reason.detail,
            };
        }
    }
    AgentVerdict {
        availability: AgentAvailability::Waitable,
        reason: ReasonCode::RuntimeOffline,
        repair: None,
        detail: format!("agent runtime is {}", rt.status),
    }
}

/// Reads the daemon's explanation off a runtime row. A row with no reason, or
/// metadata this server cannot parse, simply has no explanation — never a
/// different verdict.
fn parse_runtime_offline_reason(metadata: &serde_json::Value) -> Option<RuntimeOfflineReason> {
    if metadata.is_null() {
        return None;
    }
    #[derive(serde::Deserialize)]
    struct Envelope {
        #[serde(rename = "offline_reason")]
        offline_reason: Option<RuntimeOfflineReason>,
    }
    serde_json::from_value::<Envelope>(metadata.clone())
        .ok()
        .and_then(|e| e.offline_reason)
}

/// Durable explanation left on an issue when a trigger is refused because the
/// target's agent CLI cannot run on its machine. Two layers write it (handler
/// for a refused @mention, service for a refused assignment) — one text, one
/// place to fix. Names the repair command when reported and stays useful when
/// not: a natively installed CLI has no postinstall to re-run, and inventing
/// a command would send the user somewhere that does not exist.
pub fn runtime_unusable_notice(agent_name: &str, verdict: &AgentVerdict) -> String {
    let name = if agent_name.is_empty() {
        "The assigned agent"
    } else {
        agent_name
    };
    if let Some(repair) = &verdict.repair {
        if !repair.command.is_empty() {
            return format!(
                "{} could not start: its CLI is installed but cannot be executed on that machine, so this trigger was not queued.\n\n\
                 Usually the package's postinstall was blocked (npm 12 allowScripts, pnpm 10 approve-builds, `--ignore-scripts`, `--omit=optional`) and the bin entry is still a placeholder. On that machine, run:\n\n\
                 ```{}\n{}\n```\n\n\
                 The runtime comes back on its own within a couple of minutes; trigger the agent again after that.",
                name,
                repair_fence_language(&repair.shell),
                repair.command
            );
        }
    }
    format!(
        "{name} could not start: its CLI is installed but cannot be executed on that machine, so this trigger was not queued. \
         Reinstall the agent CLI on that machine with install scripts enabled; the runtime comes back on its own within a couple of minutes."
    )
}

/// Labels the code block with the shell the command was written for. A daemon
/// too old to report one predates Windows rendering entirely, so its command
/// is POSIX by construction.
fn repair_fence_language(shell: &str) -> &'static str {
    if shell == "powershell" {
        "powershell"
    } else {
        "bash"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    fn offline_runtime(status: &str, metadata: serde_json::Value) -> AgentRuntime {
        AgentRuntime {
            status: status.to_string(),
            metadata,
            ..test_row()
        }
    }

    // models lack Deserialize; build via a nil-filled literal helper shared
    // by tests below.
    fn test_row() -> AgentRuntime {
        AgentRuntime {
            created_at: Default::default(),
            custom_name: None,
            daemon_id: None,
            device_info: String::new(),
            id: Uuid::nil(),
            last_seen_at: None,
            legacy_daemon_id: None,
            metadata: serde_json::Value::Null,
            name: String::new(),
            owner_id: None,
            profile_id: None,
            provider: String::new(),
            runtime_mode: String::new(),
            status: String::new(),
            updated_at: Default::default(),
            visibility: String::new(),
            workspace_id: Uuid::nil(),
        }
    }

    #[test]
    fn online_runtime_is_available() {
        let v = runtime_verdict(&offline_runtime("online", serde_json::Value::Null));
        assert!(v.ready());
        assert_eq!(v.availability, AgentAvailability::Available);
    }

    #[test]
    fn plain_offline_is_waitable_with_status_detail() {
        let v = runtime_verdict(&offline_runtime("offline", json!({})));
        assert!(!v.blocked());
        assert_eq!(v.availability, AgentAvailability::Waitable);
        assert_eq!(v.reason, ReasonCode::RuntimeOffline);
        assert_eq!(v.detail, "agent runtime is offline");
    }

    #[test]
    fn not_executable_blocks_with_repair_carried_through() {
        let md = json!({"offline_reason": {"code": "not_executable", "detail": "exec format",
            "repair": {"package": "@x/cli", "command": "npm rebuild", "shell": "powershell"}}});
        let v = runtime_verdict(&offline_runtime("offline", md));
        assert!(v.blocked());
        assert_eq!(v.reason, ReasonCode::RuntimeUnusable);
        assert_eq!(v.repair.as_ref().expect("repair").command, "npm rebuild");
    }

    #[test]
    fn unparseable_metadata_is_never_a_different_verdict() {
        let v = runtime_verdict(&offline_runtime("offline", json!("garbage")));
        assert_eq!(v.availability, AgentAvailability::Waitable);
    }

    #[test]
    fn unusable_notice_renders_fence_language_and_fallback_name() {
        let mut verdict = runtime_verdict(&offline_runtime(
            "offline",
            json!({"offline_reason": {"code": "not_executable", "detail": "d",
                "repair": {"command": "pnpm rebuild"}}}),
        ));
        let notice = runtime_unusable_notice("", &verdict);
        assert!(notice.starts_with("The assigned agent could not start"));
        assert!(notice.contains("```bash\npnpm rebuild\n```"));

        verdict.repair = None;
        let notice = runtime_unusable_notice("Kimi", &verdict);
        assert!(notice.starts_with("Kimi could not start"));
        assert!(!notice.contains("```"));
    }
}
