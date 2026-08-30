//! Stable execution contract shared by every provider adapter.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Errors raised before a provider session can be handed to the daemon.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("agent executable not found: {0}")]
    ExecutableNotFound(String),
    #[error("unsupported agent runtime: {0}")]
    UnsupportedRuntime(String),
    #[error("invalid agent configuration: {0}")]
    InvalidConfig(String),
    #[error("agent protocol error: {0}")]
    Protocol(String),
    #[error("agent process error: {0}")]
    Process(#[from] std::io::Error),
}

/// Provider-neutral contract for one agent runtime family.
///
/// `execute` returns after the provider process and its protocol pumps have
/// started. The caller drains `Session.messages` and then awaits exactly one
/// terminal value from `Session.result`.
#[async_trait]
pub trait Backend: Send + Sync {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError>;
}

/// Configuration for one execution. Empty strings mean that the runtime keeps
/// its own default, matching the Go contract.
#[derive(Clone, Default)]
pub struct ExecOptions {
    pub cwd: String,
    pub model: String,
    pub system_prompt: String,
    pub thread_name: String,
    /// Non-empty only for an explicit autonomous Codex goal turn.
    pub goal_objective: String,
    pub max_turns: u32,
    pub timeout: Duration,
    pub semantic_inactivity_timeout: Duration,
    pub first_turn_no_progress_timeout: Duration,
    pub idle_watchdog_timeout: Duration,
    pub handshake_timeout: Duration,
    pub resume_session_id: String,
    pub resume_expected: bool,
    pub resume_continuity_notice: String,
    pub extra_args: Vec<String>,
    pub custom_args: Vec<String>,
    pub qwenpaw_workspace: String,
    /// `None` and `Some(Null)` mean inherit; `Some({})` is an explicitly
    /// managed empty set. See `mcp::has_managed_config`.
    pub mcp_config: Option<serde_json::Value>,
    pub thinking_level: String,
    pub service_tier: String,
    pub openclaw_mode: String,
    pub claude_settings_path: String,
    /// Cancels the provider process and its entire owned process tree. A fresh
    /// token is inert, so callers that do not need cancellation retain the Go
    /// contract's background-context behaviour.
    pub cancellation: CancellationToken,
}

impl std::fmt::Debug for ExecOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExecOptions")
            .field("cwd", &self.cwd)
            .field("model", &self.model)
            .field("thread_name", &self.thread_name)
            .field("has_goal", &!self.goal_objective.is_empty())
            .field("max_turns", &self.max_turns)
            .field("timeout", &self.timeout)
            .field(
                "semantic_inactivity_timeout",
                &self.semantic_inactivity_timeout,
            )
            .field(
                "first_turn_no_progress_timeout",
                &self.first_turn_no_progress_timeout,
            )
            .field("idle_watchdog_timeout", &self.idle_watchdog_timeout)
            .field("handshake_timeout", &self.handshake_timeout)
            .field("has_resume_session", &!self.resume_session_id.is_empty())
            .field("resume_expected", &self.resume_expected)
            .field("extra_arg_count", &self.extra_args.len())
            .field("custom_arg_count", &self.custom_args.len())
            .field("has_mcp_config", &self.mcp_config.is_some())
            .field("thinking_level", &self.thinking_level)
            .field("service_tier", &self.service_tier)
            .field("openclaw_mode", &self.openclaw_mode)
            .field("has_system_prompt", &!self.system_prompt.is_empty())
            .field(
                "has_resume_continuity_notice",
                &!self.resume_continuity_notice.is_empty(),
            )
            .finish_non_exhaustive()
    }
}

/// A running provider session.
pub struct Session {
    pub messages: mpsc::Receiver<Message>,
    pub result: oneshot::Receiver<ExecutionResult>,
}

/// Normalized event kinds consumed by the daemon Agent event history drain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MessageType {
    Text,
    Thinking,
    ToolUse,
    ToolResult,
    Status,
    Error,
    Log,
}

/// One normalized provider event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    #[serde(rename = "type")]
    pub message_type: MessageType,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub call_id: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub input: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub level: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
}

/// Token usage attributed to one model for one execution.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// Provider-authoritative cost in units of 1e-10 USD. Zero means absent.
    pub cost_usd_ticks: i64,
}

pub const COST_USD_TICKS_PER_USD: i64 = 10_000_000_000;

/// Terminal outcome. Status intentionally remains an open string: the daemon
/// adds policy statuses such as `idle_watchdog` outside provider adapters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub status: String,
    pub output: String,
    pub error: String,
    pub duration_ms: i64,
    pub session_id: String,
    pub usage: BTreeMap<String, TokenUsage>,
    /// Positive evidence that the requested resume itself was refused. False
    /// is inconclusive for providers listed by
    /// `registry::resume_rejection_undetectable`.
    pub resume_rejected: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_status_stays_open_for_daemon_policy() {
        let result = ExecutionResult {
            status: "idle_watchdog".to_string(),
            ..ExecutionResult::default()
        };
        assert_eq!(result.status, "idle_watchdog");
    }

    #[test]
    fn mcp_config_preserves_absent_null_and_empty_object() {
        let absent = ExecOptions::default();
        let null = ExecOptions {
            mcp_config: Some(serde_json::Value::Null),
            ..ExecOptions::default()
        };
        let empty = ExecOptions {
            mcp_config: Some(serde_json::json!({})),
            ..ExecOptions::default()
        };
        assert!(absent.mcp_config.is_none());
        assert_eq!(null.mcp_config, Some(serde_json::Value::Null));
        assert_eq!(empty.mcp_config, Some(serde_json::json!({})));
    }
}
