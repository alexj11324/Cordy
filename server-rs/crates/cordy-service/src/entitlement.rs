//! Cloud entitlement-policy client used by quota-bearing services.
//!
//! The client is deliberately fail-open: unavailable or malformed Cloud
//! policy disables enforcement. A bounded stale policy may still be observed,
//! but an `enforce` action is always downgraded to `observe` after expiry.

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

use crate::autopilot::{EntitlementAction, EntitlementGateDecision, EntitlementProvider};

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
    metrics: Option<Arc<cordy_metrics::BusinessMetrics>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolicyRegression {
    Policy,
    Subscription,
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
        metrics: Option<Arc<cordy_metrics::BusinessMetrics>>,
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

    fn stale(mut decision: EntitlementGateDecision) -> EntitlementGateDecision {
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
    ) -> Option<(EntitlementGateDecision, &'static str)> {
        let mut cache = self.cache.lock().await;
        cache.touch(workspace_id);
        let Some(entry) = cache.entries.get(&workspace_id) else {
            if record_outcome {
                self.record_cache("miss");
            }
            return None;
        };
        if now < entry.fresh_until {
            if record_outcome {
                self.record_cache("hit");
            }
            return Some((entry.decision.clone(), "cache_fresh"));
        }
        if entry
            .retry_after
            .is_some_and(|retry_after| now < retry_after)
        {
            if record_outcome {
                self.record_cache("retry_suppressed");
            }
            return Some(if now < entry.stale_until {
                (Self::stale(entry.decision.clone()), "stale")
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

    async fn failure_decision(&self, workspace_id: Uuid) -> (EntitlementGateDecision, bool) {
        let now = Utc::now();
        let mut cache = self.cache.lock().await;
        cache.touch(workspace_id);
        if let Some(decision) = cache
            .entries
            .get(&workspace_id)
            .filter(|entry| now < entry.stale_until)
            .map(|entry| Self::stale(entry.decision.clone()))
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
        cache.entries.insert(
            workspace_id,
            CacheEntry {
                decision: decision.clone(),
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
    ) -> EntitlementGateDecision {
        if let Some(metrics) = self.metrics.as_deref() {
            metrics.record_entitlement_decision(
                "autopilot_runs",
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
    async fn gate_autopilot_runs(&self, workspace_id: Uuid) -> EntitlementGateDecision {
        if self.emergency_disabled.load(Ordering::Acquire) {
            return self.record_decision(Self::off(), "emergency_disabled");
        }
        if workspace_id.is_nil() {
            return self.record_decision(Self::off(), "invalid_workspace");
        }
        let now = Utc::now();
        if let Some((decision, reason)) = self.cached_decision(workspace_id, now, true).await {
            return self.record_decision(decision, reason);
        }

        let refresh_lock = self.refresh_lock(workspace_id).await;
        let _guard = refresh_lock.lock().await;
        if let Some((decision, reason)) =
            self.cached_decision(workspace_id, Utc::now(), false).await
        {
            if self.emergency_disabled.load(Ordering::Acquire) {
                return self.record_decision(Self::off(), "emergency_disabled");
            }
            return self.record_decision(decision, reason);
        }
        let started = Instant::now();
        match self.fetch(workspace_id).await {
            Ok(policy) => match self.store_policy(workspace_id, policy).await {
                Ok(decision) => {
                    self.record_refresh("ok", started);
                    if self.emergency_disabled.load(Ordering::Acquire) {
                        self.record_decision(Self::off(), "emergency_disabled")
                    } else {
                        self.record_decision(decision, "refreshed")
                    }
                }
                Err(error) => {
                    let (outcome, failure_reason) =
                        if let StorePolicyError::Version(regression) = error {
                            if let Some(metrics) = self.metrics.as_deref() {
                                metrics.record_entitlement_version_regression(regression.source());
                            }
                            ("version_regression", "version_regression")
                        } else {
                            ("error", "unavailable")
                        };
                    self.record_refresh(outcome, started);
                    self.mark_failure(workspace_id).await;
                    if self.emergency_disabled.load(Ordering::Acquire) {
                        return self.record_decision(Self::off(), "emergency_disabled");
                    }
                    let (decision, stale) = self.failure_decision(workspace_id).await;
                    self.record_decision(decision, if stale { "stale" } else { failure_reason })
                }
            },
            Err(error) => {
                self.record_refresh(error.refresh_outcome(), started);
                self.mark_failure(workspace_id).await;
                if self.emergency_disabled.load(Ordering::Acquire) {
                    return self.record_decision(Self::off(), "emergency_disabled");
                }
                let (decision, stale) = self.failure_decision(workspace_id).await;
                let failure_reason = error.decision_reason();
                self.record_decision(decision, if stale { "stale" } else { failure_reason })
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
    limit: Option<i64>,
    period_start: Option<DateTime<Utc>>,
    period_end: Option<DateTime<Utc>>,
    reset_at: Option<DateTime<Utc>>,
}

struct FetchedPolicy {
    decision: EntitlementGateDecision,
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
    normalize_gate(issue_window, false)?;
    let gate = wire.gates.get("autopilot_runs").ok_or(())?;
    let (action, limit, period_start, period_end, reset_at) = normalize_gate(gate, true)?;
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
        valid_for_seconds: wire.valid_for_seconds,
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

fn normalize_gate(gate: &WireGate, periods_required: bool) -> Result<NormalizedGate, ()> {
    let action = match gate.action.as_str() {
        "off" => return Ok((EntitlementAction::Off, None, None, None, None)),
        "observe" => EntitlementAction::Observe,
        "enforce" => EntitlementAction::Enforce,
        _ => return Err(()),
    };
    let limit = gate.limit.filter(|limit| *limit >= 0).ok_or(())?;
    let supplied = [gate.period_start, gate.period_end, gate.reset_at]
        .into_iter()
        .filter(Option::is_some)
        .count();
    if supplied != 0 && supplied != 3 || periods_required && supplied != 3 {
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
        Some(limit),
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
    fn rejects_inert_or_partial_autopilot_gate() {
        let base = json!({
            "schema_version": 1,
            "policy_revision": 1,
            "subscription_version": 1,
            "valid_until": "2030-01-01T00:00:00Z",
            "valid_for_seconds": 60,
            "gates": {
                "issue_window": {"action":"off"},
                "autopilot_runs": {"action":"enforce","limit":1,"period_start":"2029-01-01T00:00:00Z"}
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
            HttpEntitlementProvider::stale(decision).gate_action,
            EntitlementAction::Observe
        );
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
