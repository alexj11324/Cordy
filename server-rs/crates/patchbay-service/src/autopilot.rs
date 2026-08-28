//! Autopilot domain service — port of `service/autopilot.go` +
//! `autopilot_quota.go`. This slice lands the types, constructor seams and
//! every pure-function anchor; the dispatch/quota/sync method families land
//! on top in the next slice (structure map: 41 functions, seven clusters).

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use sqlx::PgPool;
use uuid::Uuid;

use crate::agent_ready::{agent_readiness, AgentAvailability};
use patchbay_analytics as analytics;
use patchbay_db::dbid::new_v7;
use patchbay_db::models::{Agent, Autopilot, AutopilotRun};
use patchbay_db::queries::agent::has_active_task_for_issue;
use patchbay_db::queries::agent_invocation_target::list_agent_invocation_targets;
use patchbay_db::queries::autopilot::{
    create_autopilot_rule_version, get_autopilot, get_autopilot_run, get_autopilot_run_by_issue,
    get_autopilot_trigger, update_autopilot_last_run_at, update_autopilot_run_skipped,
    update_autopilot_run_terminal_with_quota, UpdateAutopilotRunTerminalWithQuotaRow,
};
use patchbay_db::queries::autopilot::{
    create_autopilot_run, get_autopilot_run_by_quota_reservation,
};
use patchbay_db::queries::autopilot_quota::{
    consume_autopilot_quota_reservation, create_autopilot_quota_reservation,
    ensure_autopilot_quota_period, get_autopilot_quota_period,
    get_autopilot_quota_reservation_by_key, increment_autopilot_quota_blocked,
    increment_autopilot_quota_reserved, list_recoverable_autopilot_quota_reservations,
    release_autopilot_quota_reservation,
};
use patchbay_db::queries::member::get_member_by_user_and_workspace;

use crate::dispatch_reason::ReasonCode;
use crate::task_service::{TaskService, TaskServiceError};

pub const DEFAULT_AUTOPILOT_TRIGGER_TIMEZONE: &str = "UTC";

/// Duplicate-suppression window for recently dispatched runs.
pub const AUTOPILOT_RECENT_DUPLICATE_WINDOW: Duration = Duration::from_secs(60);

/// The timezone used to render Autopilot trigger output when a trigger has no
/// configured timezone or the configured timezone fails to load. Also the
/// scheduler's default when computing next run times.
///
/// Go models this as a `*time.Location`; the label string plus a parsed
/// chrono-tz handle is the Rust equivalent.
pub fn default_trigger_location() -> (Tz, &'static str) {
    (chrono_tz::UTC, DEFAULT_AUTOPILOT_TRIGGER_TIMEZONE)
}

/// Domain service. Go's Queries/TxStarter pair collapses into one sqlx pool —
/// executor-generic queries plus `pool.begin()` cover both shapes.
#[derive(Clone)]
pub struct AutopilotService {
    pub pool: PgPool,
    pub bus: Arc<patchbay_events::Bus>,
    pub task_svc: Arc<TaskService>,
    /// Optional quota decision telemetry seam.
    pub quota_metrics: Option<Arc<dyn AutopilotQuotaMetrics>>,
    /// Cloud entitlement provider; None disables the whole quota surface
    /// (expected on self-hosted deployments).
    pub entitlements: Option<Arc<dyn EntitlementProvider>>,
}

impl AutopilotService {
    pub fn new(pool: PgPool, bus: Arc<patchbay_events::Bus>, task_svc: Arc<TaskService>) -> Self {
        Self {
            pool,
            bus,
            task_svc,
            quota_metrics: None,
            entitlements: None,
        }
    }
}

/// Accountability-bearing config snapshot stored on each rule-version for
/// audit display (PB-4302 §7). Cosmetic fields are intentionally excluded —
/// changing them does not transfer accountability.
#[derive(Debug, serde::Serialize)]
struct AutopilotRuleConfigSummary<'a> {
    assignee_type: &'a str,
    assignee_id: &'a str,
    status: &'a str,
    execution_mode: &'a str,
}

/// Appends one rule-version snapshot for a substantive publish (PB-4302
/// §3.4). Shared by handler publish paths (tx-scoped via the caller's
/// executor) and the failure monitor's system-pause. System publishers pass
/// `None` with type "system".
pub async fn record_autopilot_rule_version(
    executor: &PgPool,
    ap: &Autopilot,
    published_by_type: &str,
    published_by_id: Option<Uuid>,
) -> anyhow::Result<()> {
    let summary = AutopilotRuleConfigSummary {
        assignee_type: &ap.assignee_type,
        assignee_id: &ap.assignee_id.to_string(),
        status: &ap.status,
        execution_mode: &ap.execution_mode,
    };
    let encoded = serde_json::to_value(&summary)
        .map_err(|e| anyhow::anyhow!("marshal rule version config summary: {e}"))?;
    create_autopilot_rule_version(
        executor,
        ap.id,
        ap.workspace_id,
        published_by_type,
        published_by_id,
        &encoded,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("create autopilot rule version: no row"))?;
    Ok(())
}

// --- Entitlement seam -------------------------------------------------------

/// Mirror of the cloud entitlement gate actions the quota paths branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntitlementAction {
    Off,
    Observe,
    Enforce,
}

impl EntitlementAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Observe => "observe",
            Self::Enforce => "enforce",
        }
    }
}

/// Decision returned by the cloud entitlement gate for one gated feature.
#[derive(Debug, Clone)]
pub struct EntitlementGateDecision {
    pub gate_action: EntitlementAction,
    pub gate_limit: Option<i64>,
    pub gate_period_start: Option<DateTime<Utc>>,
    pub gate_period_end: Option<DateTime<Utc>>,
    pub gate_reset_at: Option<DateTime<Utc>>,
    pub policy_revision: i64,
    pub subscription_version: i64,
}

/// Seam standing in for Go's `entitlement.Provider`. Cloud remains the sole
/// authority over interval construction; implementations must not consult
/// local quota tables.
#[async_trait::async_trait]
pub trait EntitlementProvider: Send + Sync {
    async fn gate_autopilot_runs(&self, workspace_id: Uuid) -> EntitlementGateDecision;
}

// --- Pure predicates --------------------------------------------------------

/// A run that reached a terminal state — or whose downstream resource exists
/// (#4443 review): issue_created counts once its issue row landed, running
/// once its task row did; a stale-steal retry MUST NOT treat it as complete.
pub fn is_run_complete(run: &AutopilotRun) -> bool {
    match run.status.as_str() {
        "completed" | "failed" | "skipped" => true,
        "issue_created" => run.issue_id.is_some(),
        "running" => run.task_id.is_some(),
        _ => false,
    }
}

/// Fail-closed attribution refusal maps to attribution_blocked; everything
/// else is an unclassified internal error.
pub fn dispatch_fail_reason_code(err: &crate::task_service::TaskServiceError) -> ReasonCode {
    use crate::task_service::TaskServiceError as E;
    match err {
        E::FailClosedPolicyUnavailable(_) | E::FailClosedPolicyRead(..) | E::FailClosed(_) => {
            ReasonCode::AttributionBlocked
        }
        _ => ReasonCode::InternalError,
    }
}

pub fn task_failure_reason_for_run(task: &patchbay_db::models::AgentTaskQueue) -> String {
    if let Some(err) = &task.error {
        if !err.trim().is_empty() {
            return err.clone();
        }
    }
    if let Some(reason) = &task.failure_reason {
        if !reason.trim().is_empty() {
            return reason.clone();
        }
    }
    "task failed".to_string()
}

/// For squad autopilots the message names the squad so an operator reading
/// failure_reason knows which squad's leader is down without joining back to
/// autopilot_run.squad_id.
pub fn format_admission_reason(assignee_type: &str, raw: &str) -> String {
    let prefix = if assignee_type == "squad" {
        "squad leader "
    } else {
        "assignee "
    };
    match raw {
        "agent is archived" => format!("{prefix}agent is archived"),
        "agent has no runtime bound" => format!("{prefix}agent has no runtime bound"),
        // raw is "agent runtime is X" — surface the runtime status while
        // preserving the legacy PB-1899 suffix so alert queries do not change.
        other => format!("{other} at dispatch time"),
    }
}

/// Signals an archived squad assignee — distinct from a missing/unloadable
/// squad so the gate phrases the skip reason precisely and the failure
/// monitor does not log noise for an expected post-archive condition.
#[derive(Debug, thiserror::Error)]
#[error("squad is archived")]
pub struct ErrSquadArchived;

/// Leader-resolution failure, keeping Go's three distinguishable outcomes:
/// DB fault, known archived squad, unknown assignee_type.
#[derive(Debug, thiserror::Error)]
pub enum ResolveLeaderError {
    #[error("load squad: {0}")]
    LoadSquad(anyhow::Error),
    #[error("load squad leader: {0}")]
    LoadSquadLeader(anyhow::Error),
    #[error("load agent: {0}")]
    LoadAgent(anyhow::Error),
    #[error(transparent)]
    SquadArchived(#[from] ErrSquadArchived),
    #[error("unknown assignee_type {0:?}")]
    UnknownAssigneeType(String),
    /// pgx.ErrNoRows equivalent somewhere along the lookup chain — retrying
    /// cannot succeed (hard-deleted agent under migration 096's no-FK world,
    /// or a gone squad row).
    #[error("assignee lookup returned no row")]
    NotFound { squad_resolved: bool },
}

impl ResolveLeaderError {
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound { .. })
    }

    pub fn is_squad_archived(&self) -> bool {
        matches!(self, Self::SquadArchived(_))
    }
}

impl AutopilotService {
    /// Returns the agent that will actually execute the autopilot's work and
    /// whether the resolver took the squad branch (callers choose fail-open
    /// for transient agent-load faults vs fail-closed for a gone squad row).
    ///
    /// Archived squads are rejected here too: DeleteSquad transfers surviving
    /// autopilots to assignee_type='agent', but any row slipping through that
    /// transfer must never produce work. Unknown assignee_type values error —
    /// a CHECK constraint gates them at the DB layer, so this only fires for
    /// rows written around it.
    pub async fn resolve_leader(
        &self,
        ap: &Autopilot,
    ) -> Result<(Agent, bool), ResolveLeaderError> {
        match ap.assignee_type.as_str() {
            "" | "agent" => {
                let agent = patchbay_db::queries::agent::get_agent(&self.pool, ap.assignee_id)
                    .await
                    .map_err(ResolveLeaderError::LoadAgent)?
                    .ok_or_else(|| ResolveLeaderError::NotFound {
                        squad_resolved: false,
                    })?;
                Ok((agent, false))
            }
            "squad" => {
                let squad = patchbay_db::queries::squad::get_squad(&self.pool, ap.assignee_id)
                    .await
                    .map_err(ResolveLeaderError::LoadSquad)?
                    .ok_or_else(|| ResolveLeaderError::NotFound {
                        squad_resolved: true,
                    })?;
                if squad.archived_at.is_some() {
                    return Err(ResolveLeaderError::SquadArchived(ErrSquadArchived));
                }
                let agent = patchbay_db::queries::agent::get_agent(&self.pool, squad.leader_id)
                    .await
                    .map_err(ResolveLeaderError::LoadSquadLeader)?
                    .ok_or_else(|| ResolveLeaderError::NotFound {
                        squad_resolved: true,
                    })?;
                Ok((agent, true))
            }
            other => Err(ResolveLeaderError::UnknownAssigneeType(other.to_string())),
        }
    }

    /// Squad-id attribution hook for an autopilot_run row; only populated for
    /// assignee_type='squad' (RFC §4.e / PB-2429).
    pub fn squad_attribution(ap: &Autopilot) -> Option<Uuid> {
        if ap.assignee_type == "squad" {
            Some(ap.assignee_id)
        } else {
            None
        }
    }

    /// Analytics assignee shape; resolves the squad leader best-effort and
    /// falls back to the squad id when resolution fails.
    pub async fn assignee_analytics(&self, ap: &Autopilot) -> analytics::AutopilotAssignee {
        let mut out = analytics::AutopilotAssignee {
            assignee_type: ap.assignee_type.clone(),
            squad_id: String::new(),
            agent_id: String::new(),
        };
        if ap.assignee_type == "squad" {
            out.squad_id = ap.assignee_id.to_string();
            out.agent_id = match self.resolve_leader(ap).await {
                Ok((leader, _)) => leader.id.to_string(),
                Err(_) => ap.assignee_id.to_string(),
            };
        } else {
            out.agent_id = ap.assignee_id.to_string();
        }
        out
    }
}

pub fn autopilot_error_type(reason: &str) -> &'static str {
    if reason.contains("unknown execution_mode") {
        "configuration"
    } else if reason.starts_with("issue ") {
        "issue_terminal"
    } else if reason.contains("create issue")
        || reason.contains("enqueue task")
        || reason.contains("dispatch")
    {
        "dispatch_error"
    } else if reason.starts_with("task ") {
        "task_error"
    } else {
        "autopilot_error"
    }
}

/// Go treats a zero UUID created_by as absent ("system"); the schema column
/// is NOT NULL here, so nil plays that role.
pub fn autopilot_actor_id(ap: &Autopilot) -> String {
    let id = ap.created_by_id.to_string();
    if ap.created_by_type == "agent" && !ap.created_by_id.is_nil() {
        return format!("agent:{id}");
    }
    if ap.created_by_id.is_nil() {
        "system".to_string()
    } else {
        id
    }
}

pub fn autopilot_run_duration_ms(run: &AutopilotRun) -> i64 {
    let Some(completed_at) = run.completed_at else {
        return 0;
    };
    // triggered_at is NOT NULL in the live schema; Go's Valid fallback chain
    // collapses accordingly.
    let ms = (completed_at - run.triggered_at).num_milliseconds();
    ms.max(0)
}

// --- Trigger timezone + rendering -------------------------------------------

impl AutopilotService {
    /// Resolves the trigger's configured timezone, validating it parses; any
    /// miss falls back to UTC with a warning.
    pub async fn resolve_trigger_timezone(&self, trigger_id: Option<Uuid>) -> String {
        let Some(trigger_id) = trigger_id else {
            return DEFAULT_AUTOPILOT_TRIGGER_TIMEZONE.to_string();
        };
        let trigger = match get_autopilot_trigger(&self.pool, trigger_id).await {
            Ok(Some(t)) => t,
            Ok(None) | Err(_) => {
                tracing::warn!(trigger_id = %trigger_id, "failed to load autopilot trigger timezone; falling back to UTC");
                return DEFAULT_AUTOPILOT_TRIGGER_TIMEZONE.to_string();
            }
        };
        let Some(timezone) = trigger
            .timezone
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        else {
            return DEFAULT_AUTOPILOT_TRIGGER_TIMEZONE.to_string();
        };
        if timezone.parse::<Tz>().is_err() {
            tracing::warn!(
                trigger_id = %trigger_id,
                timezone = %timezone,
                "invalid autopilot trigger timezone; falling back to UTC"
            );
            return DEFAULT_AUTOPILOT_TRIGGER_TIMEZONE.to_string();
        }
        timezone.to_string()
    }
}

/// (location, label) for rendering; blank label defaults to UTC and an
/// unparseable zone falls back to UTC under the canonical label.
fn trigger_location(timezone: &str) -> (Tz, String) {
    let label = timezone.trim();
    let label = if label.is_empty() {
        DEFAULT_AUTOPILOT_TRIGGER_TIMEZONE
    } else {
        label
    };
    match label.parse::<Tz>() {
        Ok(tz) => (tz, label.to_string()),
        Err(_) => (
            chrono_tz::UTC,
            DEFAULT_AUTOPILOT_TRIGGER_TIMEZONE.to_string(),
        ),
    }
}

fn autopilot_run_triggered_at(run: &AutopilotRun) -> DateTime<Utc> {
    run.triggered_at
}

pub fn format_autopilot_run_timestamp(run: &AutopilotRun, timezone: &str) -> String {
    let triggered_at = autopilot_run_triggered_at(run);
    let (loc, label) = trigger_location(timezone);
    format!(
        "{} {}",
        triggered_at.with_timezone(&loc).format("%Y-%m-%d %H:%M"),
        label
    )
}

pub fn format_autopilot_run_date(run: &AutopilotRun, timezone: &str) -> String {
    let triggered_at = autopilot_run_triggered_at(run);
    let (loc, _) = trigger_location(timezone);
    triggered_at
        .with_timezone(&loc)
        .format("%Y-%m-%d")
        .to_string()
}

impl AutopilotService {
    /// User description + the rename-after-starting instruction; webhook
    /// runs additionally inline their event payload so the agent sees the
    /// event context without reading the run's trigger_payload.
    pub fn build_issue_description(
        &self,
        ap: &Autopilot,
        run: &AutopilotRun,
        trigger_timezone: &str,
    ) -> String {
        let triggered_at = format_autopilot_run_timestamp(run, trigger_timezone);
        let mut b = String::new();
        b.push_str(ap.description.as_deref().unwrap_or(""));
        b.push_str("\n\n---\n*Autopilot run triggered at ");
        b.push_str(&triggered_at);
        b.push_str(
            ". After starting work, rename this issue to accurately reflect what you are doing.*",
        );

        if run.source == "webhook" {
            if let Some(payload) = &run.trigger_payload {
                let mut event = "webhook.received".to_string();
                #[derive(serde::Deserialize)]
                struct Envelope {
                    event: Option<String>,
                    #[serde(rename = "eventPayload")]
                    event_payload: Option<serde_json::Value>,
                }
                let env: Option<Envelope> = serde_json::from_value(payload.clone()).ok();
                let pretty = env.as_ref().and_then(|env| {
                    if let Some(e) = &env.event {
                        if !e.is_empty() {
                            event = e.clone();
                        }
                    }
                    env.event_payload.as_ref().and_then(prettify_json)
                });
                let payload_json = pretty
                    .or_else(|| prettify_json(payload))
                    .unwrap_or_else(|| payload.to_string());
                b.push_str("\n\nWebhook event: ");
                b.push_str(&event);
                b.push_str("\n\nWebhook payload:\n```json\n");
                b.push_str(&payload_json);
                b.push_str("\n```");
            }
        }
        b
    }
}

/// json.MarshalIndent(v, "", "  ") equivalent.
pub fn prettify_json(raw: &serde_json::Value) -> Option<String> {
    serde_json::to_string_pretty(raw).ok()
}

// --- Issue title templates --------------------------------------------------

/// Matches any {{...}} token. Whitespace inside braces ({{ date }}) is
/// deliberately permitted; the canonical token is still {{date}}.
static ISSUE_TITLE_TEMPLATE_TOKEN_RE: std::sync::OnceLock<regex::Regex> =
    std::sync::OnceLock::new();

fn token_re() -> &'static regex::Regex {
    ISSUE_TITLE_TEMPLATE_TOKEN_RE
        .get_or_init(|| regex::Regex::new(r"\{\{\s*([^{}]*?)\s*\}\}").expect("valid"))
}

/// Placeholders interpolate_template substitutes. Keep in sync with the
/// substitution logic and the autopilots docs.
pub const SUPPORTED_ISSUE_TITLE_TEMPLATE_VARIABLES: &[&str] = &["date"];

pub fn is_supported_issue_title_variable(name: &str) -> bool {
    SUPPORTED_ISSUE_TITLE_TEMPLATE_VARIABLES.contains(&name)
}

/// Rejects templates containing any {{...}} token outside the supported set.
/// An empty template is valid (the autopilot falls back to its own Title).
/// The error names the first offending token for actionable CLI feedback.
pub fn validate_issue_title_template(tmpl: &str) -> Result<(), String> {
    if tmpl.is_empty() {
        return Ok(());
    }
    for caps in token_re().captures_iter(tmpl) {
        let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        if !is_supported_issue_title_variable(name) {
            return Err(format!(
                "unknown template variable {:?}; supported: {{{{{}}}}}",
                name,
                SUPPORTED_ISSUE_TITLE_TEMPLATE_VARIABLES.join("}}, {{")
            ));
        }
    }
    Ok(())
}

impl AutopilotService {
    /// Substitutes supported {{name}} placeholders. Whitespace inside braces
    /// tolerated so the render layer accepts everything validation accepts —
    /// otherwise users could save templates that pass validation yet emit a
    /// literal token at trigger time.
    pub fn interpolate_template(
        &self,
        ap: &Autopilot,
        run: &AutopilotRun,
        trigger_timezone: &str,
    ) -> String {
        let tmpl = match &ap.issue_title_template {
            Some(t) if !t.is_empty() => t.clone(),
            _ => ap.title.clone(),
        };
        let trigger_date = format_autopilot_run_date(run, trigger_timezone);
        token_re()
            .replace_all(&tmpl, |caps: &regex::Captures| {
                let whole = caps.get(0).map(|m| m.as_str()).unwrap_or("");
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").trim();
                match name {
                    "date" => trigger_date.clone(),
                    // Unknown names pass through untouched.
                    _ => whole.to_string(),
                }
            })
            .into_owned()
    }
}

// --- Quota types (autopilot_quota.go head) ----------------------------------

pub trait AutopilotQuotaMetrics: Send + Sync {
    fn record_autopilot_quota_decision(&self, action: &str, source: &str, result: &str);
}

impl AutopilotQuotaMetrics for patchbay_metrics::BusinessMetrics {
    fn record_autopilot_quota_decision(&self, action: &str, source: &str, result: &str) {
        patchbay_metrics::BusinessMetrics::record_autopilot_quota_decision(
            self, action, source, result,
        );
    }
}

/// Returned only for an enforce decision whose Cloud-provided interval is
/// full. HTTP callers serialize the facts without embedding commercial copy
/// or plan names in OSS.
#[derive(Debug, Clone)]
pub struct AutopilotQuotaExceededError {
    pub used: i64,
    pub reserved: i64,
    pub limit: i64,
    pub reset_at: DateTime<Utc>,
}

impl std::fmt::Display for AutopilotQuotaExceededError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("autopilot run quota exceeded")
    }
}

impl std::error::Error for AutopilotQuotaExceededError {}

/// Workspace-scoped, policy-neutral API model. A disabled/malformed decision
/// returns enabled=false and leaves all facts None.
#[derive(Debug, Clone, Default)]
pub struct AutopilotQuotaUsage {
    pub enabled: bool,
    pub action: String,
    pub used: Option<i64>,
    pub reserved: Option<i64>,
    pub limit: Option<i64>,
    pub period_start: Option<DateTime<Utc>>,
    pub period_end: Option<DateTime<Utc>>,
    pub reset_at: Option<DateTime<Utc>>,
    pub blocked_counts: std::collections::HashMap<String, i64>,
}

/// Cloud entitlement policy projection consumed by the quota paths.
#[derive(Debug, Clone)]
pub(crate) struct AutopilotQuotaPolicy {
    pub action: String,
    pub limit: i64,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub reset_at: DateTime<Utc>,
    pub policy_revision: i64,
    pub subscription_version: i64,
}

/// Random per-request key; generated only when an HTTP caller omitted theirs,
/// scoping idempotency to that single request. Go uses uuid v4; any unique
/// opaque string satisfies the contract (v7 keeps the existing dep set).
pub fn new_request_idempotency_key() -> String {
    patchbay_db::dbid::new_v7().to_string()
}

pub fn valid_autopilot_execution_source(source: &str) -> bool {
    matches!(source, "schedule" | "manual" | "webhook" | "api")
}

// --- Run creation params ----------------------------------------------------

/// What dispatch callers hand to create_run_with_quota; mirrors Go's
/// db.CreateAutopilotRunParams minus the columns the quota path injects.
#[derive(Debug, Clone, Default)]
pub struct CreateAutopilotRunParams {
    pub autopilot_id: Uuid,
    /// Nil when the run has no trigger row (manual/api).
    pub trigger_id: Uuid,
    /// Run source: schedule | manual | webhook | api.
    pub source: String,
    pub status: String,
    pub trigger_payload: serde_json::Value,
    /// Squad attribution hook; nil for agent assignees.
    pub squad_id: Uuid,
    pub planned_at: Option<DateTime<Utc>>,
    /// Nil outside the webhook delivery worker path.
    pub webhook_delivery_id: Uuid,
    pub reason_code: Option<String>,
}

async fn insert_run<'e, E>(
    executor: E,
    p: &CreateAutopilotRunParams,
    quota_reservation_id: Uuid,
) -> anyhow::Result<Option<AutopilotRun>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    create_autopilot_run(
        executor,
        p.autopilot_id,
        &p.source,
        &p.status,
        p.trigger_id,
        &p.trigger_payload,
        p.squad_id,
        p.planned_at,
        p.webhook_delivery_id,
        quota_reservation_id,
        p.reason_code.as_deref(),
        new_v7(),
    )
    .await
}

// --- Terminal transitions ---------------------------------------------------

impl AutopilotService {
    fn record_quota_decision(&self, action: &str, source: &str, result: &str) {
        if let Some(m) = &self.quota_metrics {
            m.record_autopilot_quota_decision(action, source, result);
        }
    }

    /// Maps the generated terminal-update row onto the full run model. The
    /// RETURNING clause covers every column; NOT NULL model fields unwrap
    /// with fallbacks that never fire for a row this statement just updated.
    fn run_from_terminal_row(row: UpdateAutopilotRunTerminalWithQuotaRow) -> AutopilotRun {
        AutopilotRun {
            id: row.id.unwrap_or_default(),
            autopilot_id: row.autopilot_id.unwrap_or_default(),
            trigger_id: row.trigger_id,
            source: row.source,
            status: row.status,
            issue_id: row.issue_id,
            task_id: row.task_id,
            triggered_at: row.triggered_at.unwrap_or_default(),
            completed_at: row.completed_at,
            failure_reason: row.failure_reason,
            trigger_payload: row.trigger_payload,
            result: row.result,
            created_at: row.created_at.unwrap_or_default(),
            squad_id: row.squad_id,
            planned_at: row.planned_at,
            webhook_delivery_id: row.webhook_delivery_id,
            quota_reservation_id: row.quota_reservation_id,
            reason_code: row.reason_code,
        }
    }

    /// Marks a run completed, consuming its reservation atomically.
    pub async fn complete_run(
        &self,
        run_id: Uuid,
        result: &serde_json::Value,
    ) -> anyhow::Result<AutopilotRun> {
        let row = update_autopilot_run_terminal_with_quota(
            &self.pool,
            "completed",
            result,
            None,
            None,
            run_id,
            true,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("complete autopilot run: no row"))?;
        Ok(Self::run_from_terminal_row(row))
    }

    pub async fn fail_autopilot_run(
        &self,
        run_id: Uuid,
        failure_reason: Option<&str>,
        reason_code: Option<&str>,
    ) -> anyhow::Result<AutopilotRun> {
        let row = update_autopilot_run_terminal_with_quota(
            &self.pool,
            "failed",
            &serde_json::Value::Null,
            failure_reason,
            reason_code,
            run_id,
            false,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("fail autopilot run: no row"))?;
        Ok(Self::run_from_terminal_row(row))
    }

    pub(crate) async fn skip_autopilot_run(
        &self,
        run_id: Uuid,
        failure_reason: &str,
        reason_code: Option<&str>,
    ) -> anyhow::Result<AutopilotRun> {
        let row = update_autopilot_run_terminal_with_quota(
            &self.pool,
            "skipped",
            &serde_json::Value::Null,
            Some(failure_reason),
            reason_code,
            run_id,
            false,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("skip autopilot run: no row"))?;
        Ok(Self::run_from_terminal_row(row))
    }

    pub async fn recover_partial_autopilot_run(&self, run_id: Uuid) -> anyhow::Result<bool> {
        let rows =
            patchbay_db::queries::autopilot::recover_partial_autopilot_run(&self.pool, run_id)
                .await?;
        Ok(rows.unwrap_or(0) > 0)
    }

    /// Keeps create_issue consumption immutable while releasing still-reserved
    /// run_only slots before deletion clears issue_id.
    pub async fn fail_runs_by_issue(&self, issue_id: Uuid) -> anyhow::Result<()> {
        patchbay_db::queries::autopilot::fail_autopilot_runs_by_issue(&self.pool, issue_id).await?;
        Ok(())
    }
}

/// Settles (consumes or releases) a quota reservation. `Ok(false)` means
/// nothing to do (no reservation) or terminal replay (already finalized by
/// another actor).
pub async fn settle_autopilot_quota<'e, E>(
    executor: E,
    reservation_id: Option<Uuid>,
    consume: bool,
) -> anyhow::Result<bool>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let Some(id) = reservation_id else {
        return Ok(false);
    };
    let res = if consume {
        consume_autopilot_quota_reservation(executor, id).await
    } else {
        release_autopilot_quota_reservation(executor, id).await
    };
    match res {
        // ErrNoRows equivalent: the reservation was already finalized by
        // another actor.
        Ok(None) => Ok(false),
        Ok(_) => Ok(true),
        Err(e) => Err(e),
    }
}

// --- Run-done broadcast + analytics -----------------------------------------

impl AutopilotService {
    fn publish_run_done(&self, workspace_id: &str, run: &AutopilotRun, status: &str) {
        self.bus.publish(&patchbay_events::Event {
            event_type: patchbay_protocol::EVENT_AUTOPILOT_RUN_DONE.to_string(),
            workspace_id: workspace_id.to_string(),
            actor_type: "system".to_string(),
            actor_id: String::new(),
            payload: serde_json::json!({
                "run_id": run.id.to_string(),
                "autopilot_id": run.autopilot_id.to_string(),
                "status": status,
            }),
            task_id: String::new(),
            chat_session_id: String::new(),
        });
    }

    /// For PostHog agent_id is the agent that will actually run the work (the
    /// resolved leader for squad autopilots) so per-agent task counts line up
    /// with what daemons report.
    async fn capture_issue_created_from_autopilot(
        &self,
        ap: &Autopilot,
        run: &AutopilotRun,
        issue: &patchbay_db::models::Issue,
        leader_id: Uuid,
    ) {
        let ev = analytics::issue_created(
            &autopilot_actor_id(ap),
            &ap.workspace_id.to_string(),
            &issue.id.to_string(),
            &leader_id.to_string(),
            "",
            &run.id.to_string(),
            analytics::SOURCE_AUTOPILOT,
            analytics::PLATFORM_SERVER,
        );
        patchbay_metrics::business_events::record_event(
            self.task_svc.analytics.as_deref(),
            self.task_svc.metrics.as_deref(),
            &ev,
        );
    }

    async fn capture_autopilot_run_started(
        &self,
        ap: &Autopilot,
        run: &AutopilotRun,
        trigger_source: &str,
    ) {
        // triggerSource doubles as cadence proxy (metrics/labels_pr3 note).
        let assignee = self.assignee_analytics(ap).await;
        let ev = analytics::autopilot_run_started(
            &autopilot_actor_id(ap),
            &ap.workspace_id.to_string(),
            &ap.id.to_string(),
            &run.id.to_string(),
            trigger_source,
            &assignee,
            trigger_source,
        );
        patchbay_metrics::business_events::record_event(
            self.task_svc.analytics.as_deref(),
            self.task_svc.metrics.as_deref(),
            &ev,
        );
    }

    async fn capture_autopilot_run_completed(&self, ap: &Autopilot, run: &AutopilotRun) {
        let assignee = self.assignee_analytics(ap).await;
        let ev = analytics::autopilot_run_completed(
            &autopilot_actor_id(ap),
            &ap.workspace_id.to_string(),
            &ap.id.to_string(),
            &run.id.to_string(),
            &run.source,
            &assignee,
            &run.source,
            autopilot_run_duration_ms(run),
        );
        patchbay_metrics::business_events::record_event(
            self.task_svc.analytics.as_deref(),
            self.task_svc.metrics.as_deref(),
            &ev,
        );
    }

    async fn capture_autopilot_run_failed(
        &self,
        ap: &Autopilot,
        run: &AutopilotRun,
        trigger_source: &str,
        reason: &str,
    ) {
        let reason = if reason.is_empty() { "unknown" } else { reason };
        let assignee = self.assignee_analytics(ap).await;
        let ev = analytics::autopilot_run_failed(
            &autopilot_actor_id(ap),
            &ap.workspace_id.to_string(),
            &ap.id.to_string(),
            &run.id.to_string(),
            trigger_source,
            &assignee,
            trigger_source,
            reason,
            autopilot_error_type(reason),
            false,
            autopilot_run_duration_ms(run),
        );
        patchbay_metrics::business_events::record_event(
            self.task_svc.analytics.as_deref(),
            self.task_svc.metrics.as_deref(),
            &ev,
        );
    }
}

// --- Post-admission skip machinery ------------------------------------------

/// Skip signal produced by dispatch functions after admission passed but a
/// late readiness regression caught the run.
#[derive(Debug, Clone)]
pub struct ErrDispatchSkipped {
    pub reason: String,
    pub code: ReasonCode,
}

impl std::fmt::Display for ErrDispatchSkipped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "dispatch skipped: {}", self.reason)
    }
}

impl std::error::Error for ErrDispatchSkipped {}

/// Error surface of the dispatch entry points: an admission/post-admission
/// skip or a real failure.
#[derive(Debug)]
pub enum DispatchError {
    Skipped(ErrDispatchSkipped),
    Service(TaskServiceError),
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Skipped(e) => write!(f, "{e}"),
            Self::Service(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DispatchError {}

impl From<TaskServiceError> for DispatchError {
    fn from(e: TaskServiceError) -> Self {
        Self::Service(e)
    }
}

impl AutopilotService {
    /// Recognises a skip returned by a dispatch function and rewrites the
    /// in-flight run to `skipped` instead of `failed`, returning the wire
    /// reason code on a real skip. Lives here, not inside the dispatch core:
    /// the run row is owned by the entry point up-stack and the
    /// failure-vs-skip distinction belongs to it.
    ///
    /// On update failure the run keeps its current state — the failure
    /// monitor eventually fails it out, but we never pretend it succeeded.
    pub(crate) async fn handle_dispatch_skip(
        &self,
        ap: &Autopilot,
        run: &mut AutopilotRun,
        err: &DispatchError,
    ) -> Option<ReasonCode> {
        let DispatchError::Skipped(skip_err) = err else {
            return None;
        };
        let updated = match self
            .skip_autopilot_run(run.id, &skip_err.reason, Some(skip_err.code.as_str()))
            .await
        {
            Ok(updated) => updated,
            Err(uerr) => {
                tracing::warn!(run_id = %run.id, error = %uerr, "failed to mark dispatch as skipped");
                return None;
            }
        };
        *run = updated;
        tracing::info!(
            autopilot_id = %ap.id,
            run_id = %run.id,
            reason = %skip_err.reason,
            "autopilot dispatch skipped post-admission"
        );
        // Bump last_run_at on parity with record_skipped_run (pre-flight
        // skip) and the success path: from the scheduler's/UI's point of view
        // the trigger WAS evaluated this tick.
        let _ = update_autopilot_last_run_at(&self.pool, ap.id).await;
        self.publish_run_done(&ap.workspace_id.to_string(), run, "skipped");
        Some(skip_err.code)
    }

    /// Internal-error terminal transition used by dispatch paths that cannot
    /// proceed without a classified reason.
    pub(crate) async fn fail_run(&self, run_id: Uuid, reason: &str) {
        if let Err(err) = self
            .fail_autopilot_run(
                run_id,
                Some(reason),
                Some(ReasonCode::InternalError.as_str()),
            )
            .await
        {
            tracing::warn!(run_id = %run_id, error = %err, "failed to mark autopilot run as failed");
        }
    }
}

// --- Event-listener seams (wired in cmd/server listeners) --------------------

impl AutopilotService {
    /// Updates the run when its linked create_issue issue reaches a terminal
    /// status. A custom status finalizes exactly like its canonical inherited
    /// status; the failure audit deliberately keeps issue.status so it names
    /// what a human actually chose (PB-6243).
    pub async fn sync_run_from_issue(&self, issue: &patchbay_db::models::Issue) {
        if issue.origin_type.as_deref() != Some("autopilot") {
            return;
        }
        let Some(run) = get_autopilot_run_by_issue(&self.pool, issue.id)
            .await
            .ok()
            .flatten()
        else {
            // No active run linked to this issue.
            return;
        };
        let Some(autopilot) = get_autopilot(&self.pool, run.autopilot_id)
            .await
            .ok()
            .flatten()
        else {
            return;
        };
        let ws_id = autopilot.workspace_id.to_string();

        let effective_status =
            crate::issue_status::effective(&self.pool, issue.workspace_id, &issue.status).await;
        match effective_status.as_str() {
            "done" | "in_review" => {
                let updated = match self.complete_run(run.id, &serde_json::Value::Null).await {
                    Ok(updated) => updated,
                    Err(err) => {
                        tracing::warn!(run_id = %run.id, error = %err, "failed to complete autopilot run");
                        return;
                    }
                };
                self.capture_autopilot_run_completed(&autopilot, &updated)
                    .await;
                self.publish_run_done(&ws_id, &updated, "completed");
            }
            "cancelled" | "blocked" => {
                let reason = format!("issue {}", issue.status);
                let updated = match self.fail_autopilot_run(run.id, Some(&reason), None).await {
                    Ok(updated) => updated,
                    Err(err) => {
                        tracing::warn!(run_id = %run.id, error = %err, "failed to fail autopilot run");
                        return;
                    }
                };
                let source = updated.source.clone();
                self.capture_autopilot_run_failed(&autopilot, &updated, &source, &reason)
                    .await;
                self.publish_run_done(&ws_id, &updated, "failed");
            }
            _ => {}
        }
    }

    /// Updates the run when a run_only task completes or fails.
    pub async fn sync_run_from_task(&self, task: &patchbay_db::models::AgentTaskQueue) {
        let Some(run_id) = task.autopilot_run_id else {
            return;
        };
        let Some(run) = get_autopilot_run(&self.pool, run_id).await.ok().flatten() else {
            return;
        };
        let Some(autopilot) = get_autopilot(&self.pool, run.autopilot_id)
            .await
            .ok()
            .flatten()
        else {
            return;
        };
        let ws_id = autopilot.workspace_id.to_string();

        match task.status.as_str() {
            "completed" => {
                let result = task.result.clone().unwrap_or(serde_json::Value::Null);
                let updated = match self.complete_run(run.id, &result).await {
                    Ok(updated) => updated,
                    Err(err) => {
                        tracing::warn!(run_id = %run.id, error = %err, "failed to complete autopilot run from task");
                        return;
                    }
                };
                self.capture_autopilot_run_completed(&autopilot, &updated)
                    .await;
                self.publish_run_done(&ws_id, &updated, "completed");
            }
            "failed" | "cancelled" => {
                // An empty stored error still overrides the coarse label —
                // same shape as Go's Valid check.
                let reason = match &task.error {
                    Some(e) => e.clone(),
                    None => format!("task {}", task.status),
                };
                let updated = match self.fail_autopilot_run(run.id, Some(&reason), None).await {
                    Ok(updated) => updated,
                    Err(err) => {
                        tracing::warn!(run_id = %run.id, error = %err, "failed to fail autopilot run from task");
                        return;
                    }
                };
                let source = updated.source.clone();
                self.capture_autopilot_run_failed(&autopilot, &updated, &source, &reason)
                    .await;
                self.publish_run_done(&ws_id, &updated, "failed");
            }
            _ => {}
        }
    }

    /// Fails a create_issue run when a linked-issue task terminally fails.
    /// Only create_issue runs link through issue_id (their linked issue is
    /// origin_type=autopilot by construction), so one query both identifies
    /// an in-flight run and bails ordinary issue/chat task failures.
    pub async fn sync_run_from_linked_issue_task(
        &self,
        task: &patchbay_db::models::AgentTaskQueue,
    ) {
        if task.autopilot_run_id.is_some() || task.issue_id.is_none() || task.status != "failed" {
            return;
        }
        let issue_id = task.issue_id.expect("guarded above");
        let Some(run) = get_autopilot_run_by_issue(&self.pool, issue_id)
            .await
            .ok()
            .flatten()
        else {
            return;
        };
        // A still-active task — typically the auto-retry FailTask just
        // enqueued — means the dispatch isn't terminal yet; wait for the
        // final attempt.
        match has_active_task_for_issue(&self.pool, issue_id).await {
            // No active task remains — the failure is final for this dispatch.
            Ok(Some(false)) | Ok(None) => {}
            // A still-active task (typically the auto-retry FailTask just
            // enqueued) means the dispatch isn't terminal yet.
            Ok(Some(true)) => return,
            Err(err) => {
                tracing::warn!(
                    issue_id = %issue_id,
                    task_id = %task.id,
                    error = %err,
                    "failed to check active tasks for autopilot issue failure"
                );
                return;
            }
        }
        let Some(autopilot) = get_autopilot(&self.pool, run.autopilot_id)
            .await
            .ok()
            .flatten()
        else {
            return;
        };

        let reason = task_failure_reason_for_run(task);
        let updated = match self
            .fail_autopilot_run(run.id, opt_str_reason(&reason), None)
            .await
        {
            Ok(updated) => updated,
            Err(err) => {
                tracing::warn!(
                    run_id = %run.id,
                    issue_id = %issue_id,
                    task_id = %task.id,
                    error = %err,
                    "failed to fail autopilot run from linked issue task"
                );
                return;
            }
        };
        let source = updated.source.clone();
        self.capture_autopilot_run_failed(&autopilot, &updated, &source, &reason)
            .await;
        self.publish_run_done(&autopilot.workspace_id.to_string(), &updated, "failed");
    }
}

/// Go binds FailureReason with Valid: reason != "" — empty stays NULL.
fn opt_str_reason(reason: &str) -> Option<&str> {
    if reason.is_empty() {
        None
    } else {
        Some(reason)
    }
}

// --- Quota admission / usage / reconciliation -------------------------------

/// Error surface of create_run_with_quota: a hard quota rejection carries the
/// interval facts so HTTP callers can serialize them without embedding
/// commercial copy or plan names in OSS.
#[derive(Debug, thiserror::Error)]
pub enum QuotaAdmissionError {
    #[error("invalid autopilot execution source {0:?}")]
    InvalidSource(String),
    #[error("{0}")]
    Exceeded(#[from] AutopilotQuotaExceededError),
    #[error("{0}")]
    Internal(String),
}

fn quota_admission_internal(context: &str, e: impl std::fmt::Display) -> QuotaAdmissionError {
    QuotaAdmissionError::Internal(format!("{context}: {e}"))
}

impl AutopilotService {
    /// Resolves the effective quota policy for a workspace. A malformed
    /// policy is fail-open and, critically, performs no quota-table access;
    /// Cloud remains the sole authority over interval construction.
    async fn quota_policy(&self, workspace_id: Uuid) -> Option<AutopilotQuotaPolicy> {
        let entitlements = self.entitlements.as_ref()?;
        let decision = entitlements.gate_autopilot_runs(workspace_id).await;
        if decision.gate_action == EntitlementAction::Off {
            return None;
        }
        let limit = decision.gate_limit?;
        let period_start = decision.gate_period_start?;
        let period_end = decision.gate_period_end?;
        let reset_at = decision.gate_reset_at?;
        if !matches!(
            decision.gate_action,
            EntitlementAction::Observe | EntitlementAction::Enforce
        ) || limit < 0
            || period_start >= period_end
        {
            return None;
        }
        Some(AutopilotQuotaPolicy {
            action: decision.gate_action.as_str().to_string(),
            limit,
            period_start,
            period_end,
            reset_at,
            policy_revision: decision.policy_revision,
            subscription_version: decision.subscription_version,
        })
    }

    /// Reserves and links a run in one transaction. When policy is off it
    /// intentionally uses the legacy direct INSERT so a self-hosted
    /// deployment never touches the quota tables.
    pub async fn create_run_with_quota(
        &self,
        workspace_id: Uuid,
        source: &str,
        idempotency_key: &str,
        params: &CreateAutopilotRunParams,
    ) -> Result<(AutopilotRun, bool), QuotaAdmissionError> {
        if !valid_autopilot_execution_source(source) {
            return Err(QuotaAdmissionError::InvalidSource(source.to_string()));
        }
        let Some(policy) = self.quota_policy(workspace_id).await else {
            let run = insert_run(&self.pool, params, Uuid::nil())
                .await
                .map_err(|e| quota_admission_internal("create autopilot run", e))?
                .ok_or_else(|| {
                    QuotaAdmissionError::Internal("create autopilot run: no row".into())
                })?;
            return Ok((run, false));
        };

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| quota_admission_internal("begin quota admission", e))?;
        let ps = policy.period_start;
        let pe = policy.period_end;

        // Locks (or creates) this workspace's interval row.
        let mut period = ensure_autopilot_quota_period(&mut *tx, workspace_id, Some(ps), Some(pe))
            .await
            .map_err(|e| quota_admission_internal("lock quota period", e))?
            .ok_or_else(|| QuotaAdmissionError::Internal("lock quota period: no row".into()))?;

        let existing = match get_autopilot_quota_reservation_by_key(
            &mut *tx,
            workspace_id,
            Some(ps),
            Some(pe),
            idempotency_key,
        )
        .await
        {
            Ok(found) => found,
            Err(e) => {
                return Err(quota_admission_internal("lookup quota reservation", e));
            }
        };
        if let Some(existing) = existing {
            match get_autopilot_run_by_quota_reservation(&mut *tx, existing.id).await {
                Ok(Some(run)) => {
                    tx.commit().await.map_err(|e| {
                        quota_admission_internal("commit idempotent quota admission", e)
                    })?;
                    self.record_quota_decision(&policy.action, source, "reused");
                    return Ok((run, true));
                }
                Ok(None) if existing.state == "reserved" => {
                    // The reservation/run insert normally commits atomically.
                    // Recover a manually removed or otherwise orphaned reserved
                    // row so the stable idempotency key does not wedge every
                    // retry for the whole period.
                    settle_autopilot_quota(&mut *tx, Some(existing.id), false)
                        .await
                        .map_err(|e| {
                            quota_admission_internal("release orphaned idempotency reservation", e)
                        })?;
                    period.reserved_count -= 1;
                    // Fall through to the fresh-reservation path.
                }
                Ok(None) => {
                    return Err(QuotaAdmissionError::Internal(
                        "load idempotent quota run: no run linked".into(),
                    ));
                }
                Err(e) => {
                    return Err(quota_admission_internal("load idempotent quota run", e));
                }
            }
        }

        let would_block = period.used_count + period.reserved_count >= policy.limit;
        if would_block && policy.action == EntitlementAction::Enforce.as_str() {
            increment_autopilot_quota_blocked(&mut *tx, source, workspace_id, Some(ps), Some(pe))
                .await
                .map_err(|e| quota_admission_internal("record blocked quota admission", e))?;
            tx.commit()
                .await
                .map_err(|e| quota_admission_internal("commit blocked quota admission", e))?;
            self.record_quota_decision(&policy.action, source, "blocked");
            return Err(QuotaAdmissionError::Exceeded(AutopilotQuotaExceededError {
                used: period.used_count,
                reserved: period.reserved_count,
                limit: policy.limit,
                reset_at: policy.reset_at,
            }));
        }
        // Observe-only would-blocks stay in the bounded decision metric;
        // durable blocked counts back the usage API only for decisions that
        // reject work.

        let reservation = create_autopilot_quota_reservation(
            &mut *tx,
            workspace_id,
            Some(ps),
            Some(pe),
            policy.policy_revision,
            policy.subscription_version,
            source,
            idempotency_key,
        )
        .await
        .map_err(|e| quota_admission_internal("create quota reservation", e))?
        .ok_or_else(|| QuotaAdmissionError::Internal("create quota reservation: no row".into()))?;

        increment_autopilot_quota_reserved(&mut *tx, workspace_id, Some(ps), Some(pe))
            .await
            .map_err(|e| quota_admission_internal("increment reserved quota", e))?;

        let run = insert_run(&mut *tx, params, reservation.id)
            .await
            .map_err(|e| quota_admission_internal("create quota-linked run", e))?
            .ok_or_else(|| {
                QuotaAdmissionError::Internal("create quota-linked run: no row".into())
            })?;

        tx.commit()
            .await
            .map_err(|e| quota_admission_internal("commit quota admission", e))?;
        let result = if would_block {
            "would_block"
        } else {
            "admitted"
        };
        self.record_quota_decision(&policy.action, source, result);
        Ok((run, false))
    }

    /// Workspace-scoped, policy-neutral usage model. A disabled or malformed
    /// decision returns enabled=false with every fact left unset; a period row
    /// that does not exist yet reads as zeroed counters.
    pub async fn quota_usage(&self, workspace_id: Uuid) -> anyhow::Result<AutopilotQuotaUsage> {
        let Some(policy) = self.quota_policy(workspace_id).await else {
            return Ok(AutopilotQuotaUsage::default());
        };
        let (used_count, reserved_count, blocked_value) = match get_autopilot_quota_period(
            &self.pool,
            workspace_id,
            Some(policy.period_start),
            Some(policy.period_end),
        )
        .await
        {
            Ok(Some(period)) => (
                period.used_count,
                period.reserved_count,
                period.blocked_counts,
            ),
            Ok(None) => (0, 0, serde_json::Value::Null),
            Err(e) => return Err(anyhow::anyhow!("load autopilot quota usage: {e}")),
        };
        let blocked_counts: std::collections::HashMap<String, i64> = if blocked_value.is_null() {
            std::collections::HashMap::new()
        } else {
            serde_json::from_value(blocked_value)
                .map_err(|e| anyhow::anyhow!("decode autopilot quota blocked counts: {e}"))?
        };
        Ok(AutopilotQuotaUsage {
            enabled: true,
            action: policy.action.clone(),
            used: Some(used_count),
            reserved: Some(reserved_count),
            limit: Some(policy.limit),
            period_start: Some(policy.period_start),
            period_end: Some(policy.period_end),
            reset_at: Some(policy.reset_at),
            blocked_counts,
        })
    }

    pub fn quota_enabled(&self) -> bool {
        self.entitlements.is_some()
    }

    /// Repairs crash windows left after a reservation/run transaction but
    /// before the downstream side effect or normal finalizer. The reservation
    /// transition remains CAS-based, so replicas may run this concurrently
    /// without double-adjusting counters.
    pub async fn reconcile_quota_reservations(
        &self,
        terminal_created_before: DateTime<Utc>,
        partial_created_before: DateTime<Utc>,
        limit: i32,
    ) -> anyhow::Result<usize> {
        if !self.quota_enabled() || limit <= 0 {
            return Ok(0);
        }
        let reservations = list_recoverable_autopilot_quota_reservations(
            &self.pool,
            Some(terminal_created_before),
            Some(partial_created_before),
            limit,
        )
        .await
        .context("list recoverable quota reservations")?;

        let mut settled = 0usize;
        for reservation in reservations {
            let changed = match get_autopilot_run_by_quota_reservation(&self.pool, reservation.id)
                .await
            {
                // No run ever linked: an orphaned reservation — release it.
                Ok(None) => settle_autopilot_quota(&self.pool, Some(reservation.id), false)
                    .await
                    .context("release orphan quota reservation")?,
                Err(e) => return Err(e.context("load quota-linked run")),
                Ok(Some(run)) => match run.status.as_str() {
                    "completed" => settle_autopilot_quota(&self.pool, Some(reservation.id), true)
                        .await
                        .context("consume completed quota reservation")?,
                    "failed" | "skipped" => {
                        settle_autopilot_quota(&self.pool, Some(reservation.id), false)
                            .await
                            .context("release terminal quota reservation")?
                    }
                    // Abandoned manual/api runs recover their partial state;
                    // schedule and webhook retries own their own recovery, so
                    // anything else falls through as a defensive no-op.
                    "pending" | "issue_created" | "running"
                        if matches!(run.source.as_str(), "manual" | "api")
                            && run.issue_id.is_none()
                            && run.task_id.is_none() =>
                    {
                        self.recover_partial_autopilot_run(run.id)
                            .await
                            .context("recover abandoned quota run")?
                    }
                    _ => false,
                },
            };
            if changed {
                settled += 1;
            }
        }
        Ok(settled)
    }
}

// --- Admission gate ---------------------------------------------------------

impl AutopilotService {
    /// Pre-dispatch admission check. Returns None to proceed; Some carries the
    /// human-readable failure_reason plus the wire reason code to skip with.
    ///
    /// Transient DB faults fail open (the next scheduler tick retries); the
    /// hard cases — archived/gone assignee, unready runtime, invocation denial
    /// — skip deterministically.
    pub(crate) async fn should_skip_dispatch(
        &self,
        ap: &Autopilot,
        actor_user_id: Option<Uuid>,
    ) -> Option<(String, ReasonCode)> {
        // Go checks !ap.AssigneeID.Valid; the schema column is NOT NULL here,
        // so the zero UUID plays the invalid role.
        if ap.assignee_id.is_nil() {
            return Some((
                "autopilot has no assignee".to_string(),
                ReasonCode::TargetUnavailable,
            ));
        }
        let agent = match self.resolve_leader(ap).await {
            Ok((agent, _)) => agent,
            Err(err) => {
                // Unconditional logging so ops can still spot a run of dangling
                // rows pointing at a deleted agent / archived squad.
                tracing::warn!(
                    autopilot_id = %ap.id,
                    assignee_type = %ap.assignee_type,
                    missing = err.is_not_found(),
                    archived = err.is_squad_archived(),
                    error = %err,
                    "autopilot admission: failed to resolve leader"
                );
                match err {
                    // Squad exists but is archived — DeleteSquad's transfer
                    // should have rewritten the assignee already; surfacing it
                    // keeps the reason useful when something slips past.
                    ResolveLeaderError::SquadArchived(_) => {
                        return Some((
                            "assignee squad is archived".to_string(),
                            ReasonCode::TargetUnavailable,
                        ));
                    }
                    ResolveLeaderError::NotFound {
                        squad_resolved: true,
                    } => {
                        return Some((
                            "assignee squad cannot be resolved".to_string(),
                            ReasonCode::TargetUnavailable,
                        ));
                    }
                    // Agent row hard-deleted under us — skipping beats failing
                    // open because retrying will not help either.
                    ResolveLeaderError::NotFound {
                        squad_resolved: false,
                    } => {
                        return Some((
                            "assignee agent no longer exists".to_string(),
                            ReasonCode::TargetUnavailable,
                        ));
                    }
                    // Transient DB fault — fail-open.
                    _ => return None,
                }
            }
        };

        let verdict = match agent_readiness(&self.pool, &agent).await {
            Ok(verdict) => verdict,
            Err(err) => {
                tracing::warn!(
                    autopilot_id = %ap.id,
                    runtime_id = ?agent.runtime_id,
                    error = %err,
                    "autopilot admission: failed to load runtime"
                );
                return None;
            }
        };
        if !verdict.ready() {
            // A merely-offline machine still gets create_issue work: the issue
            // is written server-side and the run waits for the laptop to come
            // back. An unusable runtime does not qualify — nothing there can
            // run until a human repairs it, so a doomed issue-create is not an
            // improvement.
            if ap.execution_mode == "create_issue"
                && verdict.availability == AgentAvailability::Waitable
            {
                tracing::info!(
                    autopilot_id = %ap.id,
                    runtime_id = ?agent.runtime_id,
                    reason = %verdict.detail,
                    "autopilot admission: allowing create_issue dispatch for offline runtime"
                );
            } else {
                return Some((
                    format_admission_reason(&ap.assignee_type, &verdict.detail),
                    verdict.reason,
                ));
            }
        }

        // Invocation gate at the autopilot layer (PB-3963 / PB-4525): a
        // MANUAL "run now" is gated by the CURRENT clicker's access, not the
        // creator's; automation falls back to the creator. Admins do NOT
        // bypass a private agent they do not own; agent-created autopilots are
        // judged as workspace principals.
        if !self.autopilot_admit_invoke(ap, &agent, actor_user_id).await {
            if actor_user_id.is_some() {
                return Some((
                    "you are not allowed to trigger this autopilot's assignee agent".to_string(),
                    ReasonCode::InvocationNotAllowed,
                ));
            }
            return Some((
                "autopilot creator lacks access to private assignee agent".to_string(),
                ReasonCode::InvocationNotAllowed,
            ));
        }
        None
    }

    /// Decides whether the dispatch's admission principal may invoke the
    /// target agent. Fail-closed on any lookup error; never grants an admin
    /// bypass.
    async fn autopilot_admit_invoke(
        &self,
        ap: &Autopilot,
        agent: &Agent,
        actor_user_id: Option<Uuid>,
    ) -> bool {
        match actor_user_id {
            Some(user_id) => {
                self.can_member_invoke_agent(agent, user_id, ap.workspace_id)
                    .await
            }
            None => self.can_creator_invoke_agent(ap, agent).await,
        }
    }

    /// Mirrors handler.canInvokeAgent with a member effective user — used for
    /// a manual autopilot "run now" where the clicker, not the creator, is
    /// the admission principal (PB-3963).
    async fn can_member_invoke_agent(
        &self,
        agent: &Agent,
        member_user_id: Uuid,
        workspace_id: Uuid,
    ) -> bool {
        // Go early-outs on an invalid member UUID; nil plays that role here
        // (and keeps a nil owner from matching a nil caller).
        if member_user_id.is_nil() {
            return false;
        }
        if agent.owner_id == Some(member_user_id) {
            return true;
        }
        if agent.permission_mode != "public_to" {
            return false;
        }
        let Ok(targets) = list_agent_invocation_targets(&self.pool, agent.id).await else {
            return false;
        };
        let is_workspace_member = matches!(
            get_member_by_user_and_workspace(&self.pool, member_user_id, workspace_id).await,
            Ok(Some(_))
        );
        for target in targets {
            match target.target_type.as_str() {
                "workspace" if is_workspace_member => return true,
                "member" if target.target_id == member_user_id => return true,
                _ => {}
            }
        }
        false
    }

    /// Automation fallback: the autopilot creator is the admission principal.
    /// Member creators must be workspace members; agent-created autopilots
    /// count as workspace-internal principals.
    async fn can_creator_invoke_agent(&self, ap: &Autopilot, agent: &Agent) -> bool {
        let creator_is_member = ap.created_by_type == "member";
        if creator_is_member && agent.owner_id == Some(ap.created_by_id) {
            return true;
        }
        if agent.permission_mode != "public_to" {
            // Private (or unknown mode): deny-by-default; only the owner
            // branch above passes. Admins and agent-created autopilots do
            // not bypass.
            return false;
        }
        let Ok(targets) = list_agent_invocation_targets(&self.pool, agent.id).await else {
            return false;
        };
        let workspace_broad = ap.created_by_type == "agent";
        let is_workspace_member = if creator_is_member {
            matches!(
                get_member_by_user_and_workspace(&self.pool, ap.created_by_id, ap.workspace_id)
                    .await,
                Ok(Some(_))
            )
        } else {
            false
        };
        for target in targets {
            match target.target_type.as_str() {
                "workspace" if is_workspace_member || workspace_broad => return true,
                "member" if creator_is_member && target.target_id == ap.created_by_id => {
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    /// Persists a pre-flight skipped run (admission refused before any run row
    /// existed), stamps the skip reason best-effort, bumps last_run_at so
    /// scheduler advancement and "last seen" UI both reflect that we did
    /// evaluate the trigger this tick, and broadcasts run-done. Deliberately
    /// does NOT touch quota tables.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn record_skipped_run(
        &self,
        autopilot: &Autopilot,
        trigger_id: Uuid,
        source: &str,
        payload: &serde_json::Value,
        planned_at: Option<DateTime<Utc>>,
        webhook_delivery_id: Uuid,
        reason: &str,
        reason_code: Option<ReasonCode>,
    ) -> anyhow::Result<AutopilotRun> {
        let code_str = reason_code.map(|c| c.as_str());
        let params = CreateAutopilotRunParams {
            autopilot_id: autopilot.id,
            trigger_id,
            source: source.to_string(),
            status: "skipped".to_string(),
            trigger_payload: payload.clone(),
            squad_id: Self::squad_attribution(autopilot).unwrap_or_else(Uuid::nil),
            planned_at,
            webhook_delivery_id,
            reason_code: code_str.map(str::to_string),
        };
        let mut run = insert_run(&self.pool, &params, Uuid::nil())
            .await
            .map_err(|e| anyhow::anyhow!("create skipped run: {e}"))?
            .ok_or_else(|| anyhow::anyhow!("create skipped run: no row"))?;

        match update_autopilot_run_skipped(&self.pool, run.id, Some(reason), code_str).await {
            Ok(Some(updated)) => run = updated,
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(
                    run_id = %run.id,
                    error = %err,
                    "failed to set skip reason on autopilot run"
                );
            }
        }

        tracing::info!(
            autopilot_id = %autopilot.id,
            run_id = %run.id,
            source = %source,
            reason = %reason,
            "autopilot dispatch skipped"
        );

        let _ = update_autopilot_last_run_at(&self.pool, autopilot.id).await;

        self.publish_run_done(&autopilot.workspace_id.to_string(), &run, "skipped");
        Ok(run)
    }
}

// --- Dispatch core -----------------------------------------------------------

// Module-tail imports kept beside their sole consumers for readability.
use crate::task_helpers::{truncate_for_summary, TRIGGER_SUMMARY_MAX_LEN};
use patchbay_db::queries::autopilot::{
    create_autopilot_task, get_autopilot_in_workspace, list_autopilot_subscribers,
    update_autopilot_run_issue_created, update_autopilot_run_running,
};
use patchbay_db::queries::inbox::create_inbox_item;
use patchbay_db::queries::subscriber::add_issue_subscriber;
use patchbay_db::queries::workspace::increment_issue_counter;

/// Outcome of a public dispatch entry point: the (possibly updated or
/// skipped) run plus the wire reason code for non-success outcomes.
pub struct DispatchOutcome {
    pub run: AutopilotRun,
    /// None on success; Some classifies skip/failure outcomes for handlers.
    pub reason_code: Option<ReasonCode>,
}

fn de(context: &'static str, e: impl std::fmt::Display) -> DispatchError {
    DispatchError::Service(TaskServiceError::Internal(format!("{context}: {e}")))
}

fn de_msg(msg: impl std::fmt::Display) -> DispatchError {
    DispatchError::Service(TaskServiceError::Internal(msg.to_string()))
}

impl AutopilotService {
    /// Shared core of the public Dispatch entry points. planned_at is the
    /// canonical UTC plan_time for scheduled triggers; manual/webhook/api
    /// pass None so the run row carries planned_at IS NULL.
    ///
    /// Divergence note: Go surfaces the schedule-source quota rejection as
    /// (skipped_run, QuotaExceeded, err); here the skipped run is persisted
    /// and returned as Ok with Some(QuotaExceeded) — the scheduler job only
    /// logs, and manual/webhook sources still surface the hard error.
    #[allow(clippy::too_many_arguments)]
    pub async fn dispatch_autopilot(
        &self,
        autopilot: &Autopilot,
        trigger_id: Uuid,
        source: &str,
        payload: &serde_json::Value,
        planned_at: Option<DateTime<Utc>>,
        webhook_delivery_id: Uuid,
        actor_user_id: Option<Uuid>,
        idempotency_key: &str,
    ) -> anyhow::Result<DispatchOutcome> {
        if let Some((reason, code)) = self.should_skip_dispatch(autopilot, actor_user_id).await {
            let run = self
                .record_skipped_run(
                    autopilot,
                    trigger_id,
                    source,
                    payload,
                    planned_at,
                    webhook_delivery_id,
                    &reason,
                    None,
                )
                .await?;
            return Ok(DispatchOutcome {
                run,
                reason_code: Some(code),
            });
        }

        // Initial status follows the execution mode.
        let initial_status = if autopilot.execution_mode == "run_only" {
            "running"
        } else {
            "issue_created"
        };

        let params = CreateAutopilotRunParams {
            autopilot_id: autopilot.id,
            trigger_id,
            source: source.to_string(),
            status: initial_status.to_string(),
            trigger_payload: payload.clone(),
            squad_id: Self::squad_attribution(autopilot).unwrap_or_else(Uuid::nil),
            planned_at,
            webhook_delivery_id,
            reason_code: None,
        };
        let (mut run, reused) = match self
            .create_run_with_quota(autopilot.workspace_id, source, idempotency_key, &params)
            .await
        {
            Ok(pair) => pair,
            Err(QuotaAdmissionError::Exceeded(quota_err)) => {
                if source == "schedule" {
                    let skipped = self
                        .record_skipped_run(
                            autopilot,
                            trigger_id,
                            source,
                            payload,
                            planned_at,
                            webhook_delivery_id,
                            &quota_err.to_string(),
                            Some(ReasonCode::QuotaExceeded),
                        )
                        .await?;
                    return Ok(DispatchOutcome {
                        run: skipped,
                        reason_code: Some(ReasonCode::QuotaExceeded),
                    });
                }
                return Err(quota_err.into());
            }
            Err(e) => return Err(anyhow::anyhow!("create run: {e}")),
        };
        if reused {
            let reason_code = reason_code_from_wire(run.reason_code.as_deref());
            return Ok(DispatchOutcome { run, reason_code });
        }
        self.capture_autopilot_run_started(autopilot, &run, source)
            .await;
        self.dispatch_autopilot_run(autopilot, trigger_id, source, &mut run, actor_user_id)
            .await
    }

    /// Downstream side effect for an already-persisted run. Creation stays
    /// separate so the webhook worker can resume the same idempotency-anchored
    /// run after a crash between run creation and issue/task creation.
    pub async fn dispatch_autopilot_run(
        &self,
        autopilot: &Autopilot,
        trigger_id: Uuid,
        source: &str,
        run: &mut AutopilotRun,
        actor_user_id: Option<Uuid>,
    ) -> anyhow::Result<DispatchOutcome> {
        match autopilot.execution_mode.as_str() {
            "create_issue" => {
                let trigger_timezone = self.resolve_trigger_timezone(Some(trigger_id)).await;
                if let Err(err) = self
                    .dispatch_create_issue(autopilot, run, &trigger_timezone, actor_user_id)
                    .await
                {
                    if let Some(code) = self.handle_dispatch_skip(autopilot, run, &err).await {
                        return Ok(DispatchOutcome {
                            run: run.clone(),
                            reason_code: Some(code),
                        });
                    }
                    let err_text = err.to_string();
                    self.fail_run(run.id, &err_text).await;
                    self.capture_autopilot_run_failed(autopilot, run, source, &err_text)
                        .await;
                    return Err(anyhow::anyhow!("dispatch create_issue: {err}"));
                }
            }
            "run_only" => {
                if let Err(err) = self.dispatch_run_only(autopilot, run, actor_user_id).await {
                    if let Some(code) = self.handle_dispatch_skip(autopilot, run, &err).await {
                        return Ok(DispatchOutcome {
                            run: run.clone(),
                            reason_code: Some(code),
                        });
                    }
                    let err_text = err.to_string();
                    self.fail_run(run.id, &err_text).await;
                    self.capture_autopilot_run_failed(autopilot, run, source, &err_text)
                        .await;
                    return Err(anyhow::anyhow!("dispatch run_only: {err}"));
                }
            }
            other => {
                let msg = format!("unknown execution_mode: {other}");
                self.fail_run(run.id, &msg).await;
                self.capture_autopilot_run_failed(autopilot, run, source, &msg)
                    .await;
                return Err(anyhow::anyhow!("{msg}"));
            }
        }

        update_autopilot_last_run_at(&self.pool, autopilot.id)
            .await
            .ok();

        self.bus.publish(&patchbay_events::Event {
            event_type: patchbay_protocol::EVENT_AUTOPILOT_RUN_START.to_string(),
            workspace_id: autopilot.workspace_id.to_string(),
            actor_type: "system".to_string(),
            actor_id: String::new(),
            payload: serde_json::json!({
                "run_id": run.id.to_string(),
                "autopilot_id": autopilot.id.to_string(),
                "source": source,
                "status": run.status,
            }),
            task_id: String::new(),
            chat_session_id: String::new(),
        });

        Ok(DispatchOutcome {
            run: run.clone(),
            reason_code: None,
        })
    }

    /// Creates the issue and enqueues the executing agent's task in one tx:
    /// recent-duplicate guard → counter/position → issue with autopilot origin
    /// → template subscriber fan-out → run link + reservation consume, commit,
    /// then issue:created broadcast, analytics, subscriber inbox rows and the
    /// actor-aware enqueue path (PB-2429 Path A / PB-4302 §4).
    async fn dispatch_create_issue(
        &self,
        ap: &Autopilot,
        run: &mut AutopilotRun,
        trigger_timezone: &str,
        actor_user_id: Option<Uuid>,
    ) -> Result<(), DispatchError> {
        let (leader, _) = self
            .resolve_leader(ap)
            .await
            .map_err(|e| de("resolve leader", e))?;

        let mut tx = self.pool.begin().await.map_err(|e| de("begin tx", e))?;

        let title = self.interpolate_template(ap, run, trigger_timezone);
        let description = self.build_issue_description(ap, run, trigger_timezone);

        // Refresh the autopilot row at dispatch time so the current project
        // binding wins over any stale snapshot the caller cached.
        let current_autopilot = get_autopilot_in_workspace(&mut *tx, ap.id, ap.workspace_id)
            .await
            .map_err(|e| de("refresh autopilot", e))?
            .ok_or_else(|| de_msg("refresh autopilot: not found"))?;
        let project_id = current_autopilot.project_id.unwrap_or_else(Uuid::nil);

        let guard_window = chrono::Duration::from_std(AUTOPILOT_RECENT_DUPLICATE_WINDOW)
            .map_err(|e| de("recent duplicate guard", e))?;
        if let (Some(duplicate), true) =
            crate::issue_guard::lock_and_find_recent_autopilot_duplicate(
                &mut tx,
                ap.workspace_id,
                Some(ap.id),
                Some(project_id).filter(|p| !p.is_nil()),
                &title,
                guard_window,
            )
            .await
            .map_err(|e| de("recent duplicate guard", e))?
        {
            return Err(DispatchError::Skipped(ErrDispatchSkipped {
                reason: format!("recent duplicate autopilot issue: {}", duplicate.id),
                code: ReasonCode::AlreadyActive,
            }));
        }

        let issue_number = increment_issue_counter(&mut *tx, ap.workspace_id)
            .await
            .map_err(|e| de("increment issue counter", e))?
            .ok_or_else(|| de_msg("increment issue counter: no row"))?;

        let new_position =
            crate::issue_position::next_top_position(&mut *tx, ap.workspace_id, "todo")
                .await
                .map_err(|e| de("get next issue position", e))?;

        // Creator is the agent that will do the work (resolved leader for
        // squads) so activity/mentions render with the right author identity;
        // the human configurer rides origin_type=autopilot + origin_id.
        let issue = patchbay_db::queries::issue::create_issue_with_origin(
            &mut *tx,
            ap.workspace_id,
            &title,
            Some(description.as_str()),
            "todo",
            "none",
            Some(ap.assignee_type.as_str()),
            Some(ap.assignee_id),
            "agent",
            leader.id,
            None,
            new_position,
            None,
            None,
            issue_number,
            (!project_id.is_nil()).then_some(project_id),
            Some("autopilot"),
            Some(ap.id),
            None,
            new_v7(),
        )
        .await
        .map_err(|e| de("create issue", e))?
        .ok_or_else(|| de_msg("create issue: no row"))?;

        // Fan out the default subscriber template inside the same tx as the
        // issue insert, BEFORE issue:created fires — notification listeners
        // then see the full subscriber set on the first event.
        let template_subs = list_autopilot_subscribers(&mut *tx, ap.id)
            .await
            .map_err(|e| de("list autopilot subscribers", e))?;
        for sub in &template_subs {
            add_issue_subscriber(&mut *tx, issue.id, &sub.user_type, sub.user_id, "autopilot")
                .await
                .map_err(|e| de("add autopilot subscriber to issue", e))?;
        }

        // Linking the run in the same tx makes the recent-duplicate guard
        // count only fully observable autopilot issues and closes the crash
        // window where recovery would see an orphan issue without its run.
        let updated_run = update_autopilot_run_issue_created(&mut *tx, run.id, issue.id)
            .await
            .map_err(|e| de("link run to issue", e))?
            .ok_or_else(|| de_msg("link run to issue: no row"))?;
        *run = updated_run;
        settle_autopilot_quota(&mut *tx, run.quota_reservation_id, true)
            .await
            .map_err(|e| de("consume quota reservation", e))?;

        tx.commit().await.map_err(|e| de("commit tx", e))?;

        // issue:created drives the existing event chain; for squad autopilots
        // this triggers shouldEnqueueSquadLeaderOnAssign downstream — no
        // separate squad-routing code lives here.
        let prefix = self.get_issue_prefix(ap.workspace_id).await;
        let effective_category =
            crate::issue_status::effective(&self.pool, ap.workspace_id, &issue.status).await;
        self.bus.publish(&patchbay_events::Event {
            event_type: patchbay_protocol::EVENT_ISSUE_CREATED.to_string(),
            workspace_id: ap.workspace_id.to_string(),
            actor_type: "agent".to_string(),
            actor_id: leader.id.to_string(),
            payload: serde_json::json!({
                "issue": crate::task_notify::issue_to_map_with_category(
                    &issue,
                    &prefix,
                    &effective_category,
                ),
            }),
            task_id: String::new(),
            chat_session_id: String::new(),
        });
        self.capture_issue_created_from_autopilot(ap, run, &issue, leader.id)
            .await;

        // Template subscribers exist by creation time here, so OQ3 says they
        // get subscription-grade inbox rows directly — after commit, so a
        // failure never rolls back the issue itself.
        self.notify_autopilot_subscribers_on_create(ap, &issue, leader.id, &template_subs)
            .await;

        // MANUAL triggers enqueue via actor-carrying entry points so
        // attribution resolves direct_human to the triggering member
        // (PB-4302 §4); automation takes the plain paths where the
        // autopilot-origin issue resolves to rule_owner.
        if ap.assignee_type == "squad" {
            if !self
                .autopilot_admit_invoke(ap, &leader, actor_user_id)
                .await
            {
                return Err(de_msg("not allowed to invoke private squad leader"));
            }
            if let Some(actor) = actor_user_id {
                self.task_svc
                    .enqueue_task_for_squad_leader_with_handoff(
                        &issue,
                        leader.id,
                        ap.assignee_id,
                        "",
                        Some(actor),
                    )
                    .await
                    .map_err(|e| de("enqueue squad leader task", e))?;
            } else {
                self.task_svc
                    .enqueue_task_for_squad_leader(&issue, leader.id, ap.assignee_id, None)
                    .await
                    .map_err(|e| de("enqueue squad leader task", e))?;
            }
        } else if let Some(actor) = actor_user_id {
            self.task_svc
                .enqueue_task_for_issue_with_handoff(&issue, "", Some(actor))
                .await
                .map_err(|e| de("enqueue task for issue", e))?;
        } else {
            self.task_svc
                .enqueue_task_for_issue(&issue, None)
                .await
                .map_err(|e| de("enqueue task for issue", e))?;
        }

        tracing::info!(
            autopilot_id = %ap.id,
            assignee_type = %ap.assignee_type,
            issue_id = %issue.id,
            leader_id = %leader.id,
            run_id = %run.id,
            "autopilot dispatched (create_issue)"
        );
        Ok(())
    }

    /// Writes one inbox row per template subscriber and broadcasts inbox:new
    /// with the listener-shaped payload. Failures log, never propagate: the
    /// issue and its subscriber rows are already committed.
    async fn notify_autopilot_subscribers_on_create(
        &self,
        ap: &Autopilot,
        issue: &patchbay_db::models::Issue,
        leader_id: Uuid,
        subscribers: &[patchbay_db::models::AutopilotSubscriber],
    ) {
        if subscribers.is_empty() {
            return;
        }
        let details = serde_json::json!({
            "autopilot_id": ap.id.to_string(),
            "reason": "autopilot",
        });
        for sub in subscribers {
            // Restricted to user_type='member' at the handler boundary;
            // defend in case that constraint relaxes (agents have no inbox).
            if sub.user_type != "member" {
                continue;
            }
            let item = match create_inbox_item(
                &self.pool,
                ap.workspace_id,
                "member",
                sub.user_id,
                "issue_subscribed",
                "info",
                Some(issue.id),
                &issue.title,
                None,
                Some("agent"),
                leader_id,
                &details,
                new_v7(),
            )
            .await
            {
                Ok(Some(item)) => item,
                Ok(None) => continue,
                Err(err) => {
                    tracing::error!(
                        autopilot_id = %ap.id,
                        issue_id = %issue.id,
                        recipient_id = %sub.user_id,
                        error = %err,
                        "autopilot subscriber inbox write failed"
                    );
                    continue;
                }
            };
            self.bus.publish(&patchbay_events::Event {
                event_type: patchbay_protocol::EVENT_INBOX_NEW.to_string(),
                workspace_id: ap.workspace_id.to_string(),
                actor_type: "agent".to_string(),
                actor_id: leader_id.to_string(),
                payload: serde_json::json!({ "item": {
                    "id": item.id.to_string(),
                    "workspace_id": item.workspace_id.to_string(),
                    "recipient_type": item.recipient_type,
                    "recipient_id": item.recipient_id.to_string(),
                    "type": item.type_,
                    "severity": item.severity,
                    "issue_id": item.issue_id.map(|i| i.to_string()),
                    "issue_status": issue.status,
                    "title": item.title,
                    "body": item.body.clone(),
                    "read": item.read,
                    "archived": item.archived,
                    "created_at": crate::task_notify::rfc3339(item.created_at),
                    "actor_type": item.actor_type.clone(),
                    "actor_id": item.actor_id.map(|a| a.to_string()),
                    "details": item.details.clone(),
                }}),
                task_id: String::new(),
                chat_session_id: String::new(),
            });
        }
    }

    /// Enqueues a direct agent task without creating an issue. Belt-and-braces
    /// gates mirror admission: a leader/runtime regression between admission
    /// and dispatch skips rather than enqueues a doomed task.
    async fn dispatch_run_only(
        &self,
        ap: &Autopilot,
        run: &mut AutopilotRun,
        actor_user_id: Option<Uuid>,
    ) -> Result<(), DispatchError> {
        let agent = match self.resolve_leader(ap).await {
            Ok((agent, _)) => agent,
            Err(err) if err.is_not_found() || err.is_squad_archived() => {
                return Err(DispatchError::Skipped(ErrDispatchSkipped {
                    reason: format_admission_reason(
                        &ap.assignee_type,
                        "assignee no longer resolvable",
                    ),
                    code: ReasonCode::TargetUnavailable,
                }));
            }
            Err(e) => return Err(de("resolve leader", e)),
        };
        let verdict = agent_readiness(&self.pool, &agent)
            .await
            .map_err(|e| de("check agent readiness", e))?;
        if !verdict.ready() {
            return Err(DispatchError::Skipped(ErrDispatchSkipped {
                reason: format_admission_reason(&ap.assignee_type, &verdict.detail),
                code: verdict.reason,
            }));
        }

        // Squad invocation gate re-check (admission principal = manual clicker,
        // else creator).
        if ap.assignee_type == "squad"
            && !self.autopilot_admit_invoke(ap, &agent, actor_user_id).await
        {
            return Err(DispatchError::Skipped(ErrDispatchSkipped {
                reason: format_admission_reason(
                    &ap.assignee_type,
                    "not allowed to invoke private squad leader",
                ),
                code: ReasonCode::InvocationNotAllowed,
            }));
        }

        // MANUAL triggers attribute direct_human to the clicker (both
        // originator and accountable, PB-4302 §4); automation resolves the
        // firing trigger's responsible human (trigger_owner → rule_owner →
        // unattributed) with evidence pinned to the run.
        let attr = match actor_user_id {
            Some(actor) => crate::attribution::direct_human_run(
                Some(actor),
                crate::attribution::evidence_autopilot_run(),
                Some(run.id),
            ),
            None => {
                crate::task_service::trigger_owner_attribution(
                    &self.pool,
                    run.trigger_id,
                    ap.workspace_id,
                    ap.id,
                    crate::attribution::evidence_autopilot_run(),
                    Some(run.id),
                )
                .await
            }
        };
        // No precise human resolved → owner_fallback, or refuse when the
        // workspace is fail-closed (PB-4302 §3.5).
        let attr = self
            .task_svc
            .apply_attribution_fallback(attr, &agent)
            .await
            .map_err(|_| {
                DispatchError::Skipped(ErrDispatchSkipped {
                    reason: format_admission_reason(
                        &ap.assignee_type,
                        "workspace fail-closed: no accountable human for autopilot run",
                    ),
                    code: ReasonCode::AttributionBlocked,
                })
            })?;

        let trigger_summary = if ap.title.is_empty() {
            None
        } else {
            Some(truncate_for_summary(&ap.title, TRIGGER_SUMMARY_MAX_LEN))
        };
        let task = create_autopilot_task(
            &self.pool,
            agent.id,
            agent.runtime_id.unwrap_or_else(Uuid::nil),
            0,
            run.id,
            // Snapshot the autopilot title so task rows self-describe later
            // without joining back to autopilot.
            trigger_summary.as_deref(),
            attr.user_id.unwrap_or_else(Uuid::nil),
            attr.accountable_user_id.unwrap_or_else(Uuid::nil),
            attr.rule_version_id.unwrap_or_else(Uuid::nil),
            attr.source.as_ref().map(|s| s.as_str()),
            attr.evidence_kind.as_ref().map(|k| k.as_str()),
            attr.evidence_ref_id.unwrap_or_else(Uuid::nil),
            new_v7(),
        )
        .await
        .map_err(|e| de("create autopilot task", e))?
        .ok_or_else(|| de_msg("create autopilot task: no row"))?;

        match update_autopilot_run_running(&self.pool, run.id, task.id).await {
            Ok(Some(updated)) => *run = updated,
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(run_id = %run.id, error = %err, "failed to update run with task_id");
            }
        }

        // Bypasses TaskService.Enqueue*, so the wakeup here is what unblocks
        // the runtime; without it a cached "empty" verdict stalls until TTL.
        self.task_svc.notify_task_enqueued(&task).await;

        tracing::info!(
            autopilot_id = %ap.id,
            task_id = %task.id,
            run_id = %run.id,
            "autopilot dispatched (run_only)"
        );
        Ok(())
    }

    async fn get_issue_prefix(&self, workspace_id: Uuid) -> String {
        match patchbay_db::queries::workspace::get_workspace(&self.pool, workspace_id).await {
            Ok(Some(ws)) => ws.issue_prefix,
            _ => String::new(),
        }
    }
}

/// Maps a stored wire string back onto the typed code for the reused-run
/// outcome; unknown/blank values read as no classification (Go passed the raw
/// string through unchecked).
fn reason_code_from_wire(wire: Option<&str>) -> Option<ReasonCode> {
    let wire = wire?;
    if wire.is_empty() {
        return None;
    }
    let all = [
        ReasonCode::TargetUnavailable,
        ReasonCode::RuntimeOffline,
        ReasonCode::RuntimeUnusable,
        ReasonCode::AgentRuntimeRequired,
        ReasonCode::AttributionBlocked,
        ReasonCode::InternalError,
        ReasonCode::AlreadyActive,
        ReasonCode::QuotaExceeded,
        ReasonCode::InvocationNotAllowed,
    ];
    all.into_iter().find(|c| c.as_str() == wire)
}

// --- Public dispatch entry points -------------------------------------------

use patchbay_db::queries::agent::list_tasks_by_issue;
use patchbay_db::queries::autopilot::{
    get_autopilot_run_by_trigger_and_planned, get_autopilot_run_by_webhook_delivery,
    get_autopilot_task_by_run,
};
use patchbay_db::queries::issue::get_issue;

impl AutopilotService {
    /// Schedule/webhook/api entry point: no member actor (rule_owner
    /// attribution), no per-run reason-code surface for a human, and no
    /// webhook delivery id — durable deliveries admit through
    /// admit_autopilot_webhook_delivery instead.
    pub async fn dispatch_autopilot_public(
        &self,
        autopilot: &Autopilot,
        trigger_id: Uuid,
        source: &str,
        payload: &serde_json::Value,
    ) -> anyhow::Result<AutopilotRun> {
        let key = format!("{source}:{}", new_request_idempotency_key());
        let outcome = self
            .dispatch_autopilot(
                autopilot,
                trigger_id,
                source,
                payload,
                None,
                Uuid::nil(),
                None,
                &key,
            )
            .await?;
        Ok(outcome.run)
    }

    /// "Run now" for a member: a direct human action attributed direct_human
    /// to the clicker across both execution modes (PB-4302 §4). A nil actor
    /// behaves exactly like source="manual" automation dispatch.
    pub async fn dispatch_autopilot_manual(
        &self,
        autopilot: &Autopilot,
        trigger_id: Uuid,
        payload: &serde_json::Value,
        actor_user_id: Option<Uuid>,
    ) -> anyhow::Result<DispatchOutcome> {
        self.dispatch_autopilot_manual_with_key(
            autopilot,
            trigger_id,
            payload,
            actor_user_id,
            &new_request_idempotency_key(),
        )
        .await
    }

    /// Preserves a caller-supplied request key so retrying the same HTTP
    /// request cannot reserve or execute twice. The manual path is the one
    /// surface that shows a per-run outcome code to a human, hence the typed
    /// reason code in the outcome.
    pub async fn dispatch_autopilot_manual_with_key(
        &self,
        autopilot: &Autopilot,
        trigger_id: Uuid,
        payload: &serde_json::Value,
        actor_user_id: Option<Uuid>,
        idempotency_key: &str,
    ) -> anyhow::Result<DispatchOutcome> {
        let key = format!("manual:{}:{idempotency_key}", autopilot.id);
        self.dispatch_autopilot(
            autopilot,
            trigger_id,
            "manual",
            payload,
            None,
            Uuid::nil(),
            actor_user_id,
            &key,
        )
        .await
    }

    /// Creates or reuses the idempotent run for a durable webhook delivery
    /// WITHOUT executing downstream issue/task side effects; HTTP keeps its
    /// 200 accepted/skipped + run_id contract while the database-backed worker
    /// still owns recoverable dispatch.
    pub async fn admit_autopilot_webhook_delivery(
        &self,
        autopilot: &Autopilot,
        trigger_id: Uuid,
        payload: &serde_json::Value,
        delivery_id: Uuid,
    ) -> anyhow::Result<AutopilotRun> {
        if delivery_id.is_nil() {
            anyhow::bail!("admit webhook delivery: delivery_id is required");
        }

        if let Some(existing) =
            get_autopilot_run_by_webhook_delivery(&self.pool, delivery_id).await?
        {
            return Ok(existing);
        }

        // Webhook admission has no member actor → automation principal
        // (rule_owner); the per-run reason code is not surfaced to a human
        // here, so it is dropped.
        if let Some((reason, _code)) = self.should_skip_dispatch(autopilot, None).await {
            let skipped = self
                .record_skipped_run(
                    autopilot,
                    trigger_id,
                    "webhook",
                    payload,
                    None,
                    delivery_id,
                    &reason,
                    None,
                )
                .await;
            return match skipped {
                Ok(run) => Ok(run),
                // A concurrent replica may have admitted the same delivery
                // while we evaluated the gate — reuse its run instead of
                // surfacing our loser error.
                Err(err) => match self
                    .recover_concurrent_webhook_admission(delivery_id)
                    .await?
                {
                    Some(run) => Ok(run),
                    None => Err(anyhow::anyhow!(
                        "admit webhook delivery: create skipped run: {err}"
                    )),
                },
            };
        }

        let initial_status = if autopilot.execution_mode == "run_only" {
            "running"
        } else {
            "issue_created"
        };
        let params = CreateAutopilotRunParams {
            autopilot_id: autopilot.id,
            trigger_id,
            source: "webhook".to_string(),
            status: initial_status.to_string(),
            trigger_payload: payload.clone(),
            squad_id: Self::squad_attribution(autopilot).unwrap_or_else(Uuid::nil),
            planned_at: None,
            webhook_delivery_id: delivery_id,
            reason_code: None,
        };
        match self
            .create_run_with_quota(
                autopilot.workspace_id,
                "webhook",
                &format!("webhook:{delivery_id}"),
                &params,
            )
            .await
        {
            Ok((run, _)) => {
                self.capture_autopilot_run_started(autopilot, &run, "webhook")
                    .await;
                Ok(run)
            }
            // Another replica may have claimed this durable delivery after our
            // admission lookup — the unique delivery/run index picks one winner
            // and the loser reuses that run. Go gates recovery on a typed 23505
            // cause; reloading unconditionally is equivalent because it only
            // ever reuses an actually-existing row.
            Err(err) => match self
                .recover_concurrent_webhook_admission(delivery_id)
                .await?
            {
                Some(run) => Ok(run),
                None => Err(anyhow::anyhow!("admit webhook delivery: create run: {err}")),
            },
        }
    }

    /// Reloads the winning concurrent run after an admission collision.
    /// Ok(None) means no winner materialized and the caller should propagate
    /// its original cause.
    async fn recover_concurrent_webhook_admission(
        &self,
        delivery_id: Uuid,
    ) -> anyhow::Result<Option<AutopilotRun>> {
        get_autopilot_run_by_webhook_delivery(&self.pool, delivery_id)
            .await
            .map_err(|e| anyhow::anyhow!("admit webhook delivery: reload concurrent run: {e}"))
    }

    /// Durable webhook worker entry point. webhook_delivery_id is persisted on
    /// the run under a partial unique index, so reclaiming a queued delivery
    /// after a crash reuses the original run instead of creating a second
    /// issue or task.
    pub async fn dispatch_autopilot_for_webhook_delivery(
        &self,
        autopilot: &Autopilot,
        trigger_id: Uuid,
        payload: &serde_json::Value,
        delivery_id: Uuid,
    ) -> anyhow::Result<AutopilotRun> {
        let mut run = self
            .admit_autopilot_webhook_delivery(autopilot, trigger_id, payload, delivery_id)
            .await?;
        if is_run_complete(&run) {
            if autopilot.execution_mode == "create_issue" && run.issue_id.is_some() {
                self.ensure_webhook_create_issue_task(autopilot, &run)
                    .await?;
            }
            return Ok(run);
        }

        // A run_only task may have committed immediately before the process
        // died while linking task_id back to the run. Repair that linkage and
        // wake the daemon; otherwise continue the same partial run below.
        if autopilot.execution_mode == "run_only" && run.task_id.is_none() {
            if let Some(repaired) = self
                .repair_autopilot_run_task_link(&run)
                .await
                .map_err(|e| anyhow::anyhow!("dispatch for webhook delivery: {e}"))?
            {
                return Ok(repaired);
            }
        }
        // Worker dispatch has no member actor; the reason code is dropped.
        let outcome = self
            .dispatch_autopilot_run(autopilot, trigger_id, "webhook", &mut run, None)
            .await?;
        Ok(outcome.run)
    }

    /// Repairs the create_issue crash window after the issue/run transaction
    /// commits but before the ordinary task enqueue does. Any existing issue
    /// task proves ownership already moved downstream; otherwise enqueue via
    /// exactly the assignee path used by the original dispatch.
    async fn ensure_webhook_create_issue_task(
        &self,
        autopilot: &Autopilot,
        run: &AutopilotRun,
    ) -> anyhow::Result<()> {
        let issue_id = run.issue_id.expect("guarded by caller");
        let tasks = list_tasks_by_issue(&self.pool, issue_id)
            .await
            .map_err(|e| {
                anyhow::anyhow!("dispatch for webhook delivery: inspect issue tasks: {e}")
            })?;
        if !tasks.is_empty() {
            return Ok(());
        }
        let issue = get_issue(&self.pool, issue_id)
            .await
            .map_err(|e| anyhow::anyhow!("dispatch for webhook delivery: load linked issue: {e}"))?
            .ok_or_else(|| {
                anyhow::anyhow!("dispatch for webhook delivery: linked issue missing")
            })?;
        let effective =
            crate::issue_status::effective(&self.pool, issue.workspace_id, &issue.status).await;
        if effective != "todo" && effective != "in_progress" {
            return Ok(());
        }
        if autopilot.assignee_type == "squad" {
            let (leader, _) = self.resolve_leader(autopilot).await.map_err(|e| {
                anyhow::anyhow!("dispatch for webhook delivery: resolve squad leader: {e}")
            })?;
            self.task_svc
                .enqueue_task_for_squad_leader(&issue, leader.id, autopilot.assignee_id, None)
                .await
                .map_err(|e| {
                    anyhow::anyhow!("dispatch for webhook delivery: repair squad task: {e}")
                })?;
            return Ok(());
        }
        self.task_svc
            .enqueue_task_for_issue(&issue, None)
            .await
            .map_err(|e| {
                anyhow::anyhow!("dispatch for webhook delivery: repair issue task: {e}")
            })?;
        Ok(())
    }

    /// Closes the run_only crash window where task creation committed but
    /// autopilot_run.task_id did not. Finding any task proves downstream
    /// ownership moved: active work is re-woken, terminal work replays through
    /// the normal finalizer instead of being duplicated.
    /// Returns the repaired run when a linked task exists.
    async fn repair_autopilot_run_task_link(
        &self,
        run: &AutopilotRun,
    ) -> anyhow::Result<Option<AutopilotRun>> {
        let Some(task) = get_autopilot_task_by_run(&self.pool, run.id).await? else {
            return Ok(None);
        };
        let mut updated = update_autopilot_run_running(&self.pool, run.id, task.id)
            .await
            .map_err(|e| anyhow::anyhow!("repair task linkage: {e}"))?
            .ok_or_else(|| anyhow::anyhow!("repair task linkage: no row"))?;
        match task.status.as_str() {
            "completed" | "failed" | "cancelled" => {
                self.sync_run_from_task(&task).await;
                updated = get_autopilot_run(&self.pool, run.id)
                    .await
                    .map_err(|e| anyhow::anyhow!("reload terminal repaired run: {e}"))?
                    .ok_or_else(|| anyhow::anyhow!("reload terminal repaired run: no row"))?;
            }
            _ => {
                self.task_svc.notify_task_enqueued(&task).await;
            }
        }
        Ok(Some(updated))
    }

    /// Scheduler bridge for one cron occurrence. Idempotent per
    /// (trigger_id, planned_at) via the partial unique index: complete runs
    /// short-circuit, partial runs are recovered (or repaired for run_only)
    /// before a fresh dispatch claims the slot. plannedAt is always a real
    /// timestamp from the scheduler, so Go's zero-time guard has no Rust
    /// counterpart.
    pub async fn dispatch_autopilot_for_plan(
        &self,
        autopilot: &Autopilot,
        trigger_id: Uuid,
        source: &str,
        payload: &serde_json::Value,
        planned_at: DateTime<Utc>,
    ) -> anyhow::Result<AutopilotRun> {
        if trigger_id.is_nil() {
            anyhow::bail!("dispatch for plan: trigger_id is required");
        }

        // Fast path: a prior attempt already created a run for this exact
        // occurrence. The unique index would also reject a duplicate INSERT,
        // but looking up front lets us short-circuit on a complete run and
        // gives us a chance to recover a partial run before retrying.
        match get_autopilot_run_by_trigger_and_planned(&self.pool, trigger_id, Some(planned_at))
            .await
        {
            Ok(Some(existing)) => {
                if is_run_complete(&existing) {
                    // Hand the complete run back so the job records SUCCESS in
                    // sys_cron_executions without duplicating any side effect.
                    return Ok(existing);
                }
                if autopilot.execution_mode == "run_only" && existing.task_id.is_none() {
                    if let Some(repaired) = self
                        .repair_autopilot_run_task_link(&existing)
                        .await
                        .map_err(|e| anyhow::anyhow!("dispatch for plan: {e}"))?
                    {
                        return Ok(repaired);
                    }
                }
                // Partial-state run from a crashed attempt. Mark it failed
                // (with a recovery reason) and release its partial-unique slot
                // so the fresh dispatch below can create a new row.
                tracing::warn!(
                    run_id = %existing.id,
                    trigger_id = %trigger_id,
                    planned_at = %patchbay_util::rfc3339_nano(planned_at),
                    status = %existing.status,
                    issue_set = existing.issue_id.is_some(),
                    task_set = existing.task_id.is_some(),
                    "autopilot dispatch for plan: recovering partial run"
                );
                let recovered = self
                    .recover_partial_autopilot_run(existing.id)
                    .await
                    .map_err(|e| anyhow::anyhow!("dispatch for plan: recover partial run: {e}"))?;
                if !recovered {
                    anyhow::bail!("dispatch for plan: partial run changed concurrently; retry");
                }
                // Fall through to a fresh dispatch below.
            }
            // No prior attempt for this occurrence.
            Ok(None) => {}
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "dispatch for plan: lookup existing run: {e}"
                ));
            }
        }

        // Scheduled dispatch has no member actor → rule_owner attribution, and
        // no human surface for a per-run reason code.
        let key = format!(
            "schedule:{}:{}",
            trigger_id,
            patchbay_util::rfc3339_nano(planned_at)
        );
        let outcome = self
            .dispatch_autopilot(
                autopilot,
                trigger_id,
                source,
                payload,
                Some(planned_at),
                Uuid::nil(),
                None,
                &key,
            )
            .await?;
        Ok(outcome.run)
    }
}

#[cfg(test)]
mod dispatch_contract_tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    async fn required_pool() -> PgPool {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL is required for autopilot dispatch contracts");
        PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect contract PostgreSQL")
    }

    async fn load_run(pool: &PgPool, autopilot_id: Uuid) -> AutopilotRun {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO autopilot_run (autopilot_id, source, status, trigger_payload) \
             VALUES ($1, 'manual', 'issue_created', '{}'::jsonb) RETURNING id",
        )
        .bind(autopilot_id)
        .fetch_one(pool)
        .await
        .expect("create dispatch run");
        get_autopilot_run(pool, id)
            .await
            .expect("load dispatch run")
            .expect("dispatch run exists")
    }

    async fn cleanup_dispatch_rows(pool: &PgPool, workspace_id: Uuid, user_id: Uuid) {
        sqlx::query(
            "DELETE FROM agent_task_queue WHERE issue_id IN \
             (SELECT id FROM issue WHERE workspace_id = $1)",
        )
        .bind(workspace_id)
        .execute(pool)
        .await
        .expect("delete dispatched tasks");
        sqlx::query(
            "DELETE FROM autopilot_run WHERE autopilot_id IN \
             (SELECT id FROM autopilot WHERE workspace_id = $1)",
        )
        .bind(workspace_id)
        .execute(pool)
        .await
        .expect("delete autopilot runs");
        sqlx::query("DELETE FROM autopilot_rule_version WHERE workspace_id = $1")
            .bind(workspace_id)
            .execute(pool)
            .await
            .expect("delete autopilot rule versions");
        for table in ["issue", "autopilot", "agent", "agent_runtime", "member"] {
            sqlx::query(&format!("DELETE FROM {table} WHERE workspace_id = $1"))
                .bind(workspace_id)
                .execute(pool)
                .await
                .unwrap_or_else(|error| panic!("delete {table}: {error}"));
        }
        sqlx::query("DELETE FROM \"user\" WHERE id = $1")
            .bind(user_id)
            .execute(pool)
            .await
            .expect("delete contract user");
        sqlx::query("DELETE FROM workspace WHERE id = $1")
            .bind(workspace_id)
            .execute(pool)
            .await
            .expect("delete contract workspace");
    }

    #[tokio::test]
    async fn production_create_issue_dispatch_uses_top_position_and_recent_duplicate_guard() {
        let pool = required_pool().await;
        let workspace_id: Uuid = sqlx::query_scalar(
            "INSERT INTO workspace (name, slug) VALUES ('autopilot dispatch contract', $1) RETURNING id",
        )
        .bind(format!("autopilot-dispatch-{}", Uuid::now_v7().simple()))
        .fetch_one(&pool)
        .await
        .expect("create contract workspace");
        let user_id: Uuid = sqlx::query_scalar(
            "INSERT INTO \"user\" (name, email) VALUES ('autopilot dispatch owner', $1) RETURNING id",
        )
        .bind(format!("autopilot-dispatch-{}@example.test", workspace_id.simple()))
        .fetch_one(&pool)
        .await
        .expect("create contract user");
        sqlx::query("INSERT INTO member (workspace_id, user_id, role) VALUES ($1, $2, 'owner')")
            .bind(workspace_id)
            .bind(user_id)
            .execute(&pool)
            .await
            .expect("create contract member");
        let runtime_id: Uuid = sqlx::query_scalar(
            "INSERT INTO agent_runtime \
             (workspace_id, daemon_id, name, runtime_mode, provider, status, last_seen_at) \
             VALUES ($1, $2, 'autopilot dispatch runtime', 'local', $3, 'online', now()) RETURNING id",
        )
        .bind(workspace_id)
        .bind(format!("autopilot-dispatch-{workspace_id}"))
        .bind(format!("autopilot-dispatch-{workspace_id}"))
        .fetch_one(&pool)
        .await
        .expect("create contract runtime");
        let agent_id: Uuid = sqlx::query_scalar(
            "INSERT INTO agent \
             (workspace_id, name, runtime_mode, status, owner_id, runtime_id) \
             VALUES ($1, 'autopilot dispatch agent', 'local', 'idle', $2, $3) RETURNING id",
        )
        .bind(workspace_id)
        .bind(user_id)
        .bind(runtime_id)
        .fetch_one(&pool)
        .await
        .expect("create contract agent");
        let autopilot_id: Uuid = sqlx::query_scalar(
            "INSERT INTO autopilot \
             (workspace_id, title, assignee_type, assignee_id, status, execution_mode, \
              issue_title_template, created_by_type, created_by_id) \
             VALUES ($1, 'recurring work', 'agent', $2, 'active', 'create_issue', \
                     'Recurring Work', 'member', $3) RETURNING id",
        )
        .bind(workspace_id)
        .bind(agent_id)
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .expect("create contract autopilot");
        let autopilot = get_autopilot(&pool, autopilot_id)
            .await
            .expect("load contract autopilot")
            .expect("contract autopilot exists");
        record_autopilot_rule_version(&pool, &autopilot, "member", Some(user_id))
            .await
            .expect("record contract rule owner");

        sqlx::query(
            "INSERT INTO issue \
             (workspace_id, title, status, priority, creator_type, creator_id, number, position) \
             VALUES ($1, 'existing top issue', 'todo', 'none', 'member', $2, 99, -40)",
        )
        .bind(workspace_id)
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("seed existing top issue");

        let bus = Arc::new(patchbay_events::Bus::new());
        let tasks = Arc::new(TaskService::new(pool.clone(), bus.clone()));
        let service = AutopilotService::new(pool.clone(), bus, tasks);
        let mut first_run = load_run(&pool, autopilot_id).await;
        let first = service
            .dispatch_autopilot_run(
                &autopilot,
                Uuid::nil(),
                "manual",
                &mut first_run,
                Some(user_id),
            )
            .await
            .expect("first production dispatch creates the issue");
        assert_eq!(first.reason_code, None);
        let first_issue_id = first.run.issue_id.expect("first dispatch issue link");
        let (title, position): (String, f64) =
            sqlx::query_as("SELECT title, position FROM issue WHERE id = $1")
                .bind(first_issue_id)
                .fetch_one(&pool)
                .await
                .expect("load first dispatched issue");
        assert_eq!(title, "Recurring Work");
        assert_eq!(position, -41.0, "dispatch must use next_top_position");
        let first_task_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM agent_task_queue WHERE issue_id = $1")
                .bind(first_issue_id)
                .fetch_one(&pool)
                .await
                .expect("count first dispatched task");
        assert_eq!(first_task_count, 1);
        let (originator_user_id, accountable_user_id, originator_source): (
            Option<Uuid>,
            Option<Uuid>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT originator_user_id, accountable_user_id, originator_source \
             FROM agent_task_queue WHERE issue_id = $1",
        )
        .bind(first_issue_id)
        .fetch_one(&pool)
        .await
        .expect("load first dispatched task attribution");
        assert_eq!(originator_user_id, Some(user_id));
        assert_eq!(accountable_user_id, Some(user_id));
        assert_eq!(originator_source.as_deref(), Some("direct_human"));

        let mut duplicate_run = load_run(&pool, autopilot_id).await;
        let duplicate = service
            .dispatch_autopilot_run(
                &autopilot,
                Uuid::nil(),
                "manual",
                &mut duplicate_run,
                Some(user_id),
            )
            .await
            .expect("recent duplicate is a classified dispatch skip");
        assert_eq!(duplicate.reason_code, Some(ReasonCode::AlreadyActive));
        assert_eq!(duplicate.run.status, "skipped");
        assert!(duplicate
            .run
            .failure_reason
            .as_deref()
            .is_some_and(|reason| reason.contains(&first_issue_id.to_string())));
        let autopilot_issue_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM issue \
             WHERE workspace_id = $1 AND origin_type = 'autopilot' AND origin_id = $2",
        )
        .bind(workspace_id)
        .bind(autopilot_id)
        .fetch_one(&pool)
        .await
        .expect("count autopilot issues after duplicate dispatch");
        assert_eq!(autopilot_issue_count, 1);

        cleanup_dispatch_rows(&pool, workspace_id, user_id).await;
    }
}
