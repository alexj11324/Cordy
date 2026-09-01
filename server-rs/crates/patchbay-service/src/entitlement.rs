//! Cloud entitlement-policy client used by quota-bearing services.
//!
//! Automation retains its existing observe-on-stale rollout behavior. Hosted
//! IM admission is fail-closed: unavailable, malformed, or stale Cloud policy
//! returns `Off`, which the task service exposes as quota unavailable.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Datelike, Timelike, Utc};
use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::Mutex;
use url::Url;
use uuid::Uuid;

use crate::automation::{EntitlementAction, EntitlementGateDecision, EntitlementProvider};

const MAX_RESPONSE_BODY: usize = 64 * 1024;
const MAX_POLICY_TTL_SECONDS: i64 = 5 * 60;
const MIN_SERVICE_TOKEN_BYTES: usize = 32;
const FAILURE_RETRY: Duration = Duration::from_secs(5);
const MAX_CACHE_ENTRIES: usize = 10_000;

#[derive(Debug, Clone)]
pub struct EntitlementClientConfig {
    pub enabled: bool,
    pub base_url: String,
    pub service_token: String,
    pub timeout: Duration,
    pub stale_grace: Duration,
    pub emergency_disabled: bool,
}

impl Default for EntitlementClientConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: String::new(),
            service_token: String::new(),
            timeout: Duration::from_secs(3),
            stale_grace: Duration::from_secs(15 * 60),
            emergency_disabled: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EntitlementClientError {
    #[error("entitlement policy base URL must be an absolute HTTP(S) URL without credentials, query, or fragment")]
    InvalidBaseUrl,
    #[error("entitlement service token must contain at least 32 non-whitespace bytes")]
    InvalidServiceToken,
    #[error("entitlement timeout must be positive and at most 3 seconds")]
    InvalidTimeout,
    #[error("failed to construct entitlement HTTP client: {0}")]
    HttpClient(reqwest::Error),
}

#[derive(Debug, Clone)]
struct CacheEntry {
    decision: EntitlementGateDecision,
    im_decision: EntitlementGateDecision,
    hosted_workspace_decision: EntitlementGateDecision,
    im_installation_decision: EntitlementGateDecision,
    fresh_until: DateTime<Utc>,
    stale_until: DateTime<Utc>,
    retry_after: Option<DateTime<Utc>>,
    last_access_sequence: u64,
}

#[derive(Debug, Default)]
struct CacheState {
    entries: HashMap<Uuid, CacheEntry>,
    refresh_locks: HashMap<Uuid, Arc<Mutex<()>>>,
    access_sequence: u64,
}

impl CacheState {
    fn next_access_sequence(&mut self) -> u64 {
        self.access_sequence = self.access_sequence.saturating_add(1);
        self.access_sequence
    }

    fn touch(&mut self, workspace_id: Uuid) {
        let sequence = self.next_access_sequence();
        if let Some(entry) = self.entries.get_mut(&workspace_id) {
            entry.last_access_sequence = sequence;
        }
    }

    fn evict_lru_if_full(&mut self, workspace_id: Uuid, max_entries: usize) {
        if self.entries.contains_key(&workspace_id) || self.entries.len() < max_entries {
            return;
        }
        if let Some(victim) = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_access_sequence)
            .map(|(workspace_id, _)| *workspace_id)
        {
            self.entries.remove(&victim);
            self.refresh_locks.remove(&victim);
        }
    }
}

pub struct HttpEntitlementProvider {
    base_url: Url,
    service_token: String,
    timeout: Duration,
    stale_grace: Duration,
    emergency_disabled: AtomicBool,
    client: reqwest::Client,
    cache: Mutex<CacheState>,
    metrics: Option<Arc<patchbay_metrics::BusinessMetrics>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolicyRegression {
    Policy,
    Subscription,
}

#[derive(Clone, Copy)]
enum GateKind {
    AutomationRuns,
    ImAgentTurns,
    HostedWorkspaceLimit,
    ImInstallationLimit,
}

impl GateKind {
    fn metric_name(self) -> &'static str {
        match self {
            Self::AutomationRuns => "automation_runs",
            Self::ImAgentTurns => "im_agent_turns",
            Self::HostedWorkspaceLimit => "hosted_workspace_limit",
            Self::ImInstallationLimit => "im_installation_limit",
        }
    }
}

enum StorePolicyError {
    Version(PolicyRegression),
    InvalidStaleGrace,
}

#[derive(Clone, Copy)]
enum FetchPolicyError {
    Timeout,
    Network,
    Unauthorized,
    NotFound,
    Server,
    Client,
    Status,
    Read,
    InvalidPolicy,
}

impl FetchPolicyError {
    fn refresh_outcome(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Network => "network",
            Self::Unauthorized => "unauthorized",
            Self::NotFound => "not_found",
            Self::Server => "5xx",
            Self::Client => "4xx",
            Self::Status => "status",
            Self::Read => "read",
            Self::InvalidPolicy => "invalid_policy",
        }
    }

    fn decision_reason(self) -> &'static str {
        match self {
            Self::InvalidPolicy => "invalid_policy",
            _ => "unavailable",
        }
    }
}

impl PolicyRegression {
    fn source(self) -> &'static str {
        match self {
            Self::Policy => "policy",
            Self::Subscription => "subscription",
        }
    }
}

impl HttpEntitlementProvider {
    pub fn new(
        config: EntitlementClientConfig,
    ) -> Result<Option<Arc<Self>>, EntitlementClientError> {
        Self::new_with_metrics(config, None)
    }

    pub fn new_with_metrics(
        config: EntitlementClientConfig,
        metrics: Option<Arc<patchbay_metrics::BusinessMetrics>>,
    ) -> Result<Option<Arc<Self>>, EntitlementClientError> {
        if !config.enabled {
            return Ok(None);
        }
        let base_url = Url::parse(config.base_url.trim())
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https"))
            .filter(|url| url.host_str().is_some())
            .filter(|url| url.username().is_empty() && url.password().is_none())
            .filter(|url| url.query().is_none() && url.fragment().is_none())
            .ok_or(EntitlementClientError::InvalidBaseUrl)?;
        if config.service_token.trim() != config.service_token
            || config
                .service_token
                .bytes()
                .any(|byte| byte.is_ascii_whitespace())
            || config.service_token.len() < MIN_SERVICE_TOKEN_BYTES
        {
            return Err(EntitlementClientError::InvalidServiceToken);
        }
        if config.timeout.is_zero() || config.timeout > Duration::from_secs(3) {
            return Err(EntitlementClientError::InvalidTimeout);
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(config.timeout)
            .build()
            .map_err(EntitlementClientError::HttpClient)?;
        Ok(Some(Arc::new(Self {
            base_url,
            service_token: config.service_token,
            timeout: config.timeout,
            stale_grace: config.stale_grace,
            emergency_disabled: AtomicBool::new(config.emergency_disabled),
            client,
            cache: Mutex::new(CacheState::default()),
            metrics,
        })))
    }

    pub fn set_emergency_disabled(&self, disabled: bool) {
        self.emergency_disabled.store(disabled, Ordering::Release);
    }

    fn off() -> EntitlementGateDecision {
        EntitlementGateDecision {
            gate_action: EntitlementAction::Off,
            gate_limit: None,
            gate_period_start: None,
            gate_period_end: None,
            gate_reset_at: None,
            policy_revision: 0,
            subscription_version: 0,
        }
    }

    fn stale(mut decision: EntitlementGateDecision, kind: GateKind) -> EntitlementGateDecision {
        if !matches!(kind, GateKind::AutomationRuns) {
            return Self::off();
        }
        if decision.gate_action == EntitlementAction::Enforce {
            decision.gate_action = EntitlementAction::Observe;
        }
        decision
    }

    async fn cached_decision(
        &self,
        workspace_id: Uuid,
        now: DateTime<Utc>,
        record_outcome: bool,
        kind: GateKind,
    ) -> Option<(EntitlementGateDecision, &'static str)> {
        let mut cache = self.cache.lock().await;
        cache.touch(workspace_id);
        let Some(entry) = cache.entries.get(&workspace_id) else {
            if record_outcome {
                self.record_cache("miss");
            }
            return None;
        };
        let decision = match kind {
            GateKind::AutomationRuns => &entry.decision,
            GateKind::ImAgentTurns => &entry.im_decision,
            GateKind::HostedWorkspaceLimit => &entry.hosted_workspace_decision,
            GateKind::ImInstallationLimit => &entry.im_installation_decision,
        };
        if now < entry.fresh_until {
            if record_outcome {
                self.record_cache("hit");
            }
            return Some((decision.clone(), "cache_fresh"));
        }
        if entry
            .retry_after
            .is_some_and(|retry_after| now < retry_after)
        {
            if record_outcome {
                self.record_cache("retry_suppressed");
            }
            return Some(if now < entry.stale_until {
                (Self::stale(decision.clone(), kind), "stale")
            } else {
                (Self::off(), "unavailable")
            });
        }
        if record_outcome {
            self.record_cache("expired");
        }
        None
    }

    async fn refresh_lock(&self, workspace_id: Uuid) -> Arc<Mutex<()>> {
        let mut cache = self.cache.lock().await;
        cache
            .refresh_locks
            .entry(workspace_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    async fn mark_failure(&self, workspace_id: Uuid) {
        let mut cache = self.cache.lock().await;
        let now = Utc::now();
        cache.evict_lru_if_full(workspace_id, MAX_CACHE_ENTRIES);
        let access_sequence = cache.next_access_sequence();
        let entry = cache
            .entries
            .entry(workspace_id)
            .or_insert_with(|| CacheEntry {
                decision: Self::off(),
                im_decision: Self::off(),
                hosted_workspace_decision: Self::off(),
                im_installation_decision: Self::off(),
                fresh_until: now,
                stale_until: now,
                retry_after: None,
                last_access_sequence: access_sequence,
            });
        entry.retry_after = Some(
            now + chrono::Duration::from_std(FAILURE_RETRY)
                .expect("five seconds fits chrono duration"),
        );
        entry.last_access_sequence = access_sequence;
    }

    async fn failure_decision(
        &self,
        workspace_id: Uuid,
        kind: GateKind,
    ) -> (EntitlementGateDecision, bool) {
        let now = Utc::now();
        let mut cache = self.cache.lock().await;
        cache.touch(workspace_id);
        if let Some(decision) = cache
            .entries
            .get(&workspace_id)
            .filter(|entry| now < entry.stale_until)
            .map(|entry| {
                let decision = match kind {
                    GateKind::AutomationRuns => &entry.decision,
                    GateKind::ImAgentTurns => &entry.im_decision,
                    GateKind::HostedWorkspaceLimit => &entry.hosted_workspace_decision,
                    GateKind::ImInstallationLimit => &entry.im_installation_decision,
                };
                Self::stale(decision.clone(), kind)
            })
        {
            (decision, true)
        } else {
            (Self::off(), false)
        }
    }

    async fn store_policy(
        &self,
        workspace_id: Uuid,
        fetched: FetchedPolicy,
    ) -> Result<EntitlementGateDecision, StorePolicyError> {
        let now = Utc::now();
        let ttl = chrono::Duration::seconds(fetched.valid_for_seconds);
        let stale_grace = chrono::Duration::from_std(self.stale_grace)
            .map_err(|_| StorePolicyError::InvalidStaleGrace)?;
        let mut cache = self.cache.lock().await;
        if let Some(current) = cache.entries.get(&workspace_id) {
            if now < current.stale_until {
                if fetched.decision.policy_revision < current.decision.policy_revision {
                    return Err(StorePolicyError::Version(PolicyRegression::Policy));
                }
                if fetched.decision.subscription_version < current.decision.subscription_version {
                    return Err(StorePolicyError::Version(PolicyRegression::Subscription));
                }
            }
        }
        cache.evict_lru_if_full(workspace_id, MAX_CACHE_ENTRIES);
        let last_access_sequence = cache.next_access_sequence();
        let decision = fetched.decision;
        let im_decision = fetched.im_decision;
        let hosted_workspace_decision = fetched.hosted_workspace_decision;
        let im_installation_decision = fetched.im_installation_decision;
        cache.entries.insert(
            workspace_id,
            CacheEntry {
                decision: decision.clone(),
                im_decision,
                hosted_workspace_decision,
                im_installation_decision,
                fresh_until: now + ttl,
                stale_until: now + ttl + stale_grace,
                retry_after: None,
                last_access_sequence,
            },
        );
        Ok(decision)
    }

    fn record_cache(&self, outcome: &str) {
        if let Some(metrics) = self.metrics.as_deref() {
            metrics.record_entitlement_cache(outcome);
        }
    }

    fn record_refresh(&self, outcome: &str, started: Instant) {
        if let Some(metrics) = self.metrics.as_deref() {
            metrics.record_entitlement_refresh(outcome, started.elapsed().as_secs_f64());
        }
    }

    fn record_decision(
        &self,
        decision: EntitlementGateDecision,
        reason: &'static str,
        kind: GateKind,
    ) -> EntitlementGateDecision {
        if let Some(metrics) = self.metrics.as_deref() {
            metrics.record_entitlement_decision(
                kind.metric_name(),
                decision.gate_action.as_str(),
                reason,
            );
        }
        decision
    }

    async fn fetch(&self, workspace_id: Uuid) -> Result<FetchedPolicy, FetchPolicyError> {
        let mut url = self.base_url.clone();
        let prefix = self.base_url.path().trim_end_matches('/');
        url.set_path(&format!(
            "{prefix}/api/v1/internal/entitlement-policies/{workspace_id}"
        ));
        url.set_query(None);
        url.set_fragment(None);

        let response = tokio::time::timeout(
            self.timeout,
            self.client
                .get(url)
                .header(reqwest::header::ACCEPT, "application/json")
                .bearer_auth(&self.service_token)
                .send(),
        )
        .await
        .map_err(|_| FetchPolicyError::Timeout)?
        .map_err(|error| {
            if error.is_timeout() {
                FetchPolicyError::Timeout
            } else {
                FetchPolicyError::Network
            }
        })?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(match response.status() {
                reqwest::StatusCode::UNAUTHORIZED => FetchPolicyError::Unauthorized,
                reqwest::StatusCode::NOT_FOUND => FetchPolicyError::NotFound,
                status if status.is_server_error() => FetchPolicyError::Server,
                status if status.is_client_error() => FetchPolicyError::Client,
                _ => FetchPolicyError::Status,
            });
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BODY as u64)
        {
            return Err(FetchPolicyError::InvalidPolicy);
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| FetchPolicyError::Read)?;
            if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BODY {
                return Err(FetchPolicyError::InvalidPolicy);
            }
            body.extend_from_slice(&chunk);
        }
        let wire: WirePolicy =
            serde_json::from_slice(&body).map_err(|_| FetchPolicyError::InvalidPolicy)?;
        normalize_policy(wire).map_err(|_| FetchPolicyError::InvalidPolicy)
    }
}

#[async_trait]
impl EntitlementProvider for HttpEntitlementProvider {
    async fn gate_automation_runs(&self, workspace_id: Uuid) -> EntitlementGateDecision {
        self.gate(workspace_id, GateKind::AutomationRuns).await
    }

    async fn gate_im_agent_turns(&self, workspace_id: Uuid) -> EntitlementGateDecision {
        self.gate(workspace_id, GateKind::ImAgentTurns).await
    }

    async fn gate_hosted_workspace_limit(&self, workspace_id: Uuid) -> EntitlementGateDecision {
        self.gate(workspace_id, GateKind::HostedWorkspaceLimit)
            .await
    }

    async fn gate_im_installation_limit(&self, workspace_id: Uuid) -> EntitlementGateDecision {
        self.gate(workspace_id, GateKind::ImInstallationLimit).await
    }
}

impl HttpEntitlementProvider {
    async fn gate(&self, workspace_id: Uuid, kind: GateKind) -> EntitlementGateDecision {
        if self.emergency_disabled.load(Ordering::Acquire) {
            return self.record_decision(Self::off(), "emergency_disabled", kind);
        }
        if workspace_id.is_nil() {
            return self.record_decision(Self::off(), "invalid_workspace", kind);
        }
        let now = Utc::now();
        if let Some((decision, reason)) = self.cached_decision(workspace_id, now, true, kind).await
        {
            return self.record_decision(decision, reason, kind);
        }

        let refresh_lock = self.refresh_lock(workspace_id).await;
        let _guard = refresh_lock.lock().await;
        if let Some((decision, reason)) = self
            .cached_decision(workspace_id, Utc::now(), false, kind)
            .await
        {
            if self.emergency_disabled.load(Ordering::Acquire) {
                return self.record_decision(Self::off(), "emergency_disabled", kind);
            }
            return self.record_decision(decision, reason, kind);
        }
        let started = Instant::now();
        match self.fetch(workspace_id).await {
            Ok(policy) => {
                let decision = match kind {
                    GateKind::AutomationRuns => policy.decision.clone(),
                    GateKind::ImAgentTurns => policy.im_decision.clone(),
                    GateKind::HostedWorkspaceLimit => policy.hosted_workspace_decision.clone(),
                    GateKind::ImInstallationLimit => policy.im_installation_decision.clone(),
                };
                match self.store_policy(workspace_id, policy).await {
                    Ok(_) => {
                        self.record_refresh("ok", started);
                        if self.emergency_disabled.load(Ordering::Acquire) {
                            self.record_decision(Self::off(), "emergency_disabled", kind)
                        } else {
                            self.record_decision(decision, "refreshed", kind)
                        }
                    }
                    Err(error) => {
                        let (outcome, failure_reason) =
                            if let StorePolicyError::Version(regression) = error {
                                if let Some(metrics) = self.metrics.as_deref() {
                                    metrics
                                        .record_entitlement_version_regression(regression.source());
                                }
                                ("version_regression", "version_regression")
                            } else {
                                ("error", "unavailable")
                            };
                        self.record_refresh(outcome, started);
                        self.mark_failure(workspace_id).await;
                        if self.emergency_disabled.load(Ordering::Acquire) {
                            return self.record_decision(Self::off(), "emergency_disabled", kind);
                        }
                        let (decision, stale) = self.failure_decision(workspace_id, kind).await;
                        self.record_decision(
                            decision,
                            if stale { "stale" } else { failure_reason },
                            kind,
                        )
                    }
                }
            }
            Err(error) => {
                self.record_refresh(error.refresh_outcome(), started);
                self.mark_failure(workspace_id).await;
                if self.emergency_disabled.load(Ordering::Acquire) {
                    return self.record_decision(Self::off(), "emergency_disabled", kind);
                }
                let (decision, stale) = self.failure_decision(workspace_id, kind).await;
                let failure_reason = error.decision_reason();
                self.record_decision(decision, if stale { "stale" } else { failure_reason }, kind)
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct WirePolicy {
    schema_version: i32,
    policy_revision: i64,
    subscription_version: i64,
    valid_until: DateTime<Utc>,
    valid_for_seconds: i64,
    gates: HashMap<String, WireGate>,
}

#[derive(Debug, Clone, Deserialize)]
struct WireGate {
    action: String,
    /// `None` means the field was omitted; `Some(None)` is an explicit JSON
    /// null and is the only representation of an unlimited IM gate.
    #[serde(default, deserialize_with = "deserialize_double_option_i64")]
    limit: Option<Option<i64>>,
    period_start: Option<DateTime<Utc>>,
    period_end: Option<DateTime<Utc>>,
    reset_at: Option<DateTime<Utc>>,
}

/// Preserve the distinction between an omitted JSON field and an explicit
/// `null`. Serde's default nested `Option` implementation maps both forms to
/// `None`, but entitlement policy uses the distinction to make unlimited IM
/// capacity an explicit, auditable decision rather than an accidental default.
fn deserialize_double_option_i64<'de, D>(
    deserializer: D,
) -> Result<Option<Option<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<i64>::deserialize(deserializer)?))
}

struct FetchedPolicy {
    decision: EntitlementGateDecision,
    im_decision: EntitlementGateDecision,
    hosted_workspace_decision: EntitlementGateDecision,
    im_installation_decision: EntitlementGateDecision,
    valid_for_seconds: i64,
}

fn normalize_policy(wire: WirePolicy) -> Result<FetchedPolicy, ()> {
    if wire.schema_version != 1
        || wire.policy_revision <= 0
        || wire.subscription_version < 0
        || is_zero_time(wire.valid_until)
        || !(1..=MAX_POLICY_TTL_SECONDS).contains(&wire.valid_for_seconds)
    {
        return Err(());
    }
    let issue_window = wire.gates.get("issue_window").ok_or(())?;
    normalize_gate(issue_window, false, false)?;
    let gate = wire.gates.get("automation_runs").ok_or(())?;
    let (action, limit, period_start, period_end, reset_at) = normalize_gate(gate, true, false)?;
    let im_decision = wire
        .gates
        .get("im_agent_turns")
        .map(|gate| {
            let (action, limit, period_start, period_end, reset_at) =
                normalize_gate(gate, true, true)?;
            Ok::<EntitlementGateDecision, ()>(EntitlementGateDecision {
                gate_action: action,
                gate_limit: limit,
                gate_period_start: period_start,
                gate_period_end: period_end,
                gate_reset_at: reset_at,
                policy_revision: wire.policy_revision,
                subscription_version: wire.subscription_version,
            })
        })
        .transpose()?
        .unwrap_or_else(EntitlementGateDecision::off);
    let hosted_workspace_decision = wire
        .gates
        .get("hosted_workspace_limit")
        .map(|gate| {
            normalize_capacity_decision(gate, wire.policy_revision, wire.subscription_version)
        })
        .transpose()?
        .unwrap_or_else(EntitlementGateDecision::off);
    let im_installation_decision = wire
        .gates
        .get("im_installation_limit")
        .map(|gate| {
            normalize_capacity_decision(gate, wire.policy_revision, wire.subscription_version)
        })
        .transpose()?
        .unwrap_or_else(EntitlementGateDecision::off);
    Ok(FetchedPolicy {
        decision: EntitlementGateDecision {
            gate_action: action,
            gate_limit: limit,
            gate_period_start: period_start,
            gate_period_end: period_end,
            gate_reset_at: reset_at,
            policy_revision: wire.policy_revision,
            subscription_version: wire.subscription_version,
        },
        im_decision,
        hosted_workspace_decision,
        im_installation_decision,
        valid_for_seconds: wire.valid_for_seconds,
    })
}

fn normalize_capacity_decision(
    gate: &WireGate,
    policy_revision: i64,
    subscription_version: i64,
) -> Result<EntitlementGateDecision, ()> {
    if gate.period_start.is_some() || gate.period_end.is_some() || gate.reset_at.is_some() {
        return Err(());
    }
    let (gate_action, gate_limit, _, _, _) = normalize_gate(gate, false, true)?;
    Ok(EntitlementGateDecision {
        gate_action,
        gate_limit,
        gate_period_start: None,
        gate_period_end: None,
        gate_reset_at: None,
        policy_revision,
        subscription_version,
    })
}

fn is_zero_time(value: DateTime<Utc>) -> bool {
    value.year() == 1
        && value.ordinal() == 1
        && value.num_seconds_from_midnight() == 0
        && value.nanosecond() == 0
}

type NormalizedGate = (
    EntitlementAction,
    Option<i64>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
);

fn normalize_gate(
    gate: &WireGate,
    periods_required: bool,
    allow_unlimited: bool,
) -> Result<NormalizedGate, ()> {
    let action = match gate.action.as_str() {
        "off" => return Ok((EntitlementAction::Off, None, None, None, None)),
        "observe" => EntitlementAction::Observe,
        "enforce" => EntitlementAction::Enforce,
        _ => return Err(()),
    };
    let limit = match gate.limit {
        Some(Some(limit)) if limit >= 0 => Some(limit),
        Some(None) if allow_unlimited => None,
        // An omitted limit is malformed, even for a gate that allows an
        // explicit unlimited value. Keeping omission distinct from null
        // prevents an invalid policy from silently becoming unlimited.
        _ => return Err(()),
    };
    let supplied = [gate.period_start, gate.period_end, gate.reset_at]
        .into_iter()
        .filter(Option::is_some)
        .count();
    if limit.is_none() && supplied != 0
        || limit.is_some() && supplied != 0 && supplied != 3
        || limit.is_some() && periods_required && supplied != 3
    {
        return Err(());
    }
    if supplied == 3 {
        let start = gate.period_start.ok_or(())?;
        let end = gate.period_end.ok_or(())?;
        let reset = gate.reset_at.ok_or(())?;
        if start >= end || start >= reset {
            return Err(());
        }
    }
    Ok((
        action,
        limit,
        gate.period_start,
        gate.period_end,
        gate.reset_at,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fetch_failures_use_bounded_go_compatible_metric_labels() {
        let cases = [
            (FetchPolicyError::Timeout, "timeout", "unavailable"),
            (FetchPolicyError::Network, "network", "unavailable"),
            (
                FetchPolicyError::Unauthorized,
                "unauthorized",
                "unavailable",
            ),
            (FetchPolicyError::NotFound, "not_found", "unavailable"),
            (FetchPolicyError::Server, "5xx", "unavailable"),
            (FetchPolicyError::Client, "4xx", "unavailable"),
            (FetchPolicyError::Status, "status", "unavailable"),
            (FetchPolicyError::Read, "read", "unavailable"),
            (
                FetchPolicyError::InvalidPolicy,
                "invalid_policy",
                "invalid_policy",
            ),
        ];
        for (error, refresh, decision) in cases {
            assert_eq!(error.refresh_outcome(), refresh);
            assert_eq!(error.decision_reason(), decision);
        }
    }

    #[test]
    fn rejects_inert_or_partial_automation_gate() {
        let base = json!({
            "schema_version": 1,
            "policy_revision": 1,
            "subscription_version": 1,
            "valid_until": "2030-01-01T00:00:00Z",
            "valid_for_seconds": 60,
            "gates": {
                "issue_window": {"action":"off"},
                "automation_runs": {"action":"enforce","limit":1,"period_start":"2029-01-01T00:00:00Z"}
            }
        });
        assert!(normalize_policy(serde_json::from_value(base).unwrap()).is_err());
    }

    #[test]
    fn stale_enforcement_is_downgraded_to_observe() {
        let decision = EntitlementGateDecision {
            gate_action: EntitlementAction::Enforce,
            gate_limit: Some(1),
            gate_period_start: None,
            gate_period_end: None,
            gate_reset_at: None,
            policy_revision: 1,
            subscription_version: 1,
        };
        assert_eq!(
            HttpEntitlementProvider::stale(decision, GateKind::AutomationRuns).gate_action,
            EntitlementAction::Observe
        );
    }

    #[test]
    fn stale_im_policy_is_unavailable_instead_of_fail_open() {
        let decision = EntitlementGateDecision {
            gate_action: EntitlementAction::Enforce,
            gate_limit: Some(100),
            gate_period_start: None,
            gate_period_end: None,
            gate_reset_at: None,
            policy_revision: 1,
            subscription_version: 1,
        };
        assert_eq!(
            HttpEntitlementProvider::stale(decision, GateKind::ImAgentTurns).gate_action,
            EntitlementAction::Off
        );
    }

    #[test]
    fn im_policy_accepts_null_as_an_explicit_unlimited_entitlement() {
        let wire: WirePolicy = serde_json::from_value(json!({
            "schema_version": 1,
            "policy_revision": 1,
            "subscription_version": 1,
            "valid_until": "2030-01-01T00:00:00Z",
            "valid_for_seconds": 60,
            "gates": {
                "issue_window": {"action":"off"},
                "automation_runs": {
                    "action":"enforce",
                    "limit":1,
                    "period_start":"2029-01-01T00:00:00Z",
                    "period_end":"2029-02-01T00:00:00Z",
                    "reset_at":"2029-02-01T00:00:00Z"
                },
                "im_agent_turns": {"action":"enforce","limit":null},
                "hosted_workspace_limit": {"action":"enforce","limit":null},
                "im_installation_limit": {"action":"enforce","limit":null}
            }
        }))
        .unwrap();
        let policy = normalize_policy(wire).unwrap();
        assert_eq!(policy.im_decision.gate_action, EntitlementAction::Enforce);
        assert_eq!(policy.im_decision.gate_limit, None);
        assert_eq!(
            policy.hosted_workspace_decision.gate_action,
            EntitlementAction::Enforce
        );
        assert_eq!(policy.hosted_workspace_decision.gate_limit, None);
        assert_eq!(policy.im_installation_decision.gate_limit, None);
    }

    #[test]
    fn capacity_gates_accept_limits_but_reject_period_windows() {
        let wire: WirePolicy = serde_json::from_value(json!({
            "schema_version": 1,
            "policy_revision": 2,
            "subscription_version": 3,
            "valid_until": "2030-01-01T00:00:00Z",
            "valid_for_seconds": 60,
            "gates": {
                "issue_window": {"action":"off"},
                "automation_runs": {
                    "action":"enforce",
                    "limit":1,
                    "period_start":"2029-01-01T00:00:00Z",
                    "period_end":"2029-02-01T00:00:00Z",
                    "reset_at":"2029-02-01T00:00:00Z"
                },
                "hosted_workspace_limit": {"action":"enforce","limit":2},
                "im_installation_limit": {"action":"enforce","limit":1}
            }
        }))
        .unwrap();
        let policy = normalize_policy(wire).expect("valid capacity gates");
        assert_eq!(policy.hosted_workspace_decision.gate_limit, Some(2));
        assert_eq!(policy.im_installation_decision.gate_limit, Some(1));

        let gate: WireGate = serde_json::from_value(json!({
            "action":"enforce",
            "limit":2,
            "period_start":"2029-01-01T00:00:00Z"
        }))
        .unwrap();
        assert!(normalize_capacity_decision(&gate, 1, 1).is_err());
    }

    #[test]
    fn im_policy_rejects_an_omitted_limit() {
        let wire: WirePolicy = serde_json::from_value(json!({
            "schema_version": 1,
            "policy_revision": 1,
            "subscription_version": 1,
            "valid_until": "2030-01-01T00:00:00Z",
            "valid_for_seconds": 60,
            "gates": {
                "issue_window": {"action":"off"},
                "automation_runs": {
                    "action":"enforce",
                    "limit":1,
                    "period_start":"2029-01-01T00:00:00Z",
                    "period_end":"2029-02-01T00:00:00Z",
                    "reset_at":"2029-02-01T00:00:00Z"
                },
                "im_agent_turns": {"action":"enforce"}
            }
        }))
        .unwrap();
        assert!(normalize_policy(wire).is_err());
    }

    #[test]
    fn enabled_client_rejects_weak_credentials() {
        let config = EntitlementClientConfig {
            enabled: true,
            base_url: "https://cloud.example".into(),
            service_token: "short".into(),
            ..Default::default()
        };
        assert!(matches!(
            HttpEntitlementProvider::new(config),
            Err(EntitlementClientError::InvalidServiceToken)
        ));
    }

    #[test]
    fn policy_cache_evicts_the_least_recently_used_workspace() {
        let mut cache = CacheState::default();
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        let incoming = Uuid::from_u128(3);
        let now = Utc::now();
        let entry = |last_access_sequence| CacheEntry {
            decision: HttpEntitlementProvider::off(),
            im_decision: HttpEntitlementProvider::off(),
            hosted_workspace_decision: HttpEntitlementProvider::off(),
            im_installation_decision: HttpEntitlementProvider::off(),
            fresh_until: now,
            stale_until: now,
            retry_after: None,
            last_access_sequence,
        };
        cache.entries.insert(first, entry(1));
        cache.entries.insert(second, entry(2));
        cache.access_sequence = 2;

        cache.touch(first);
        cache.evict_lru_if_full(incoming, 2);

        assert!(cache.entries.contains_key(&first));
        assert!(!cache.entries.contains_key(&second));
    }
}
