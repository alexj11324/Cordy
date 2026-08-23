//! Task lifecycle and LLM usage metrics — port of
//! `server/internal/metrics/business.go`.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use prometheus::{Counter, CounterVec, Gauge, GaugeVec, HistogramOpts, HistogramVec, Opts};

use crate::business_events::BusinessEventMetrics;
use crate::labels::{
    metric_labels, normalize_failure_reason, normalize_runtime_mode, normalize_runtime_provider,
    normalize_task_source, normalize_terminal_status, normalize_token_type,
    validate_business_metric_labels,
};
use crate::pricing::{price_for_model_alias, token_cost_usd, COST_USD_TICKS_PER_USD};

pub(crate) const TASK_DURATION_BUCKETS: &[f64] = &[
    1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1200.0, 3600.0, 7200.0,
];

pub(crate) const CHAT_CLAIM_RESUME_QUERY_DURATION_BUCKETS: &[f64] = &[
    0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
];

#[derive(Debug, Clone)]
struct ActiveTaskLabels {
    source: String,
    runtime_mode: String,
}

pub struct BusinessMetrics {
    task_enqueued: CounterVec,
    task_dispatched: CounterVec,
    task_started: CounterVec,
    task_terminal: CounterVec,
    task_failed: CounterVec,
    task_queue_wait: HistogramVec,
    task_run_seconds: HistogramVec,
    task_total_seconds: HistogramVec,
    task_in_progress: GaugeVec,
    task_iterations: HistogramVec,

    llm_tokens: CounterVec,
    llm_cost_usd: CounterVec,
    llm_unpriced_tokens: CounterVec,
    llm_requests: CounterVec,

    task_queued_expired: CounterVec,
    task_lease_expired: CounterVec,
    chat_claim_session_fallback_needed: Counter,
    chat_claim_session_fallback_result: CounterVec,
    chat_claim_resume_query_duration: HistogramVec,
    runtime_gc_deleted: Counter,
    runtime_gc_failed: Counter,
    runtime_gc_blocked: Gauge,
    runtime_gc_blocked_observation_failed: Counter,
    entitlement_config_error: Counter,
    entitlement_cache: CounterVec,
    entitlement_refresh: CounterVec,
    entitlement_refresh_duration: HistogramVec,
    entitlement_decision: CounterVec,
    entitlement_version_regression: CounterVec,
    autopilot_quota_decision: CounterVec,
    autopilot_failure_monitor: CounterVec,

    active_tasks: Mutex<HashMap<String, ActiveTaskLabels>>,

    // PR3 funnel / community / commercial counters.
    pub(crate) events: BusinessEventMetrics,
}

fn counter_vec(name: &'static str, help: &'static str) -> CounterVec {
    CounterVec::new(Opts::new(name, help), metric_labels(name)).expect("valid counter vec")
}

fn histogram_vec(name: &'static str, help: &'static str, buckets: &[f64]) -> HistogramVec {
    HistogramVec::new(
        HistogramOpts::new(name, help).buckets(buckets.to_vec()),
        metric_labels(name),
    )
    .expect("valid histogram vec")
}

impl BusinessMetrics {
    pub fn new() -> Self {
        validate_business_metric_labels();
        let m = Self {
            task_enqueued: counter_vec(
                "cordy_agent_task_enqueued_total",
                "Total agent tasks enqueued.",
            ),
            task_dispatched: counter_vec(
                "cordy_agent_task_dispatched_total",
                "Total agent tasks dispatched to a runtime.",
            ),
            task_started: counter_vec(
                "cordy_agent_task_started_total",
                "Total agent tasks that reached running state.",
            ),
            task_terminal: counter_vec(
                "cordy_agent_task_terminal_total",
                "Total agent tasks that reached a terminal state.",
            ),
            task_failed: counter_vec(
                "cordy_agent_task_failed_total",
                "Total failed agent tasks by canonical failure reason.",
            ),
            task_queue_wait: histogram_vec(
                "cordy_agent_task_queue_wait_seconds",
                "Time agent tasks spent queued before dispatch.",
                TASK_DURATION_BUCKETS,
            ),
            task_run_seconds: histogram_vec(
                "cordy_agent_task_run_seconds",
                "Time agent tasks spent running before a terminal state.",
                TASK_DURATION_BUCKETS,
            ),
            task_total_seconds: histogram_vec(
                "cordy_agent_task_total_seconds",
                "Total time from agent task creation to terminal state.",
                TASK_DURATION_BUCKETS,
            ),
            task_in_progress: GaugeVec::new(
                Opts::new(
                    "cordy_agent_task_in_progress",
                    "Current agent tasks dispatched by this process and not yet terminal.",
                ),
                metric_labels("cordy_agent_task_in_progress"),
            )
            .expect("valid gauge vec"),
            task_iterations: histogram_vec(
                "cordy_agent_task_iteration_count",
                "Retry attempt count observed when an agent task reaches a terminal state.",
                &[1.0, 2.0, 3.0, 4.0, 5.0, 10.0],
            ),
            llm_tokens: counter_vec(
                "cordy_llm_tokens_total",
                "Total priced LLM tokens by provider, model, token type, runtime mode, and task source.",
            ),
            llm_cost_usd: counter_vec(
                "cordy_llm_cost_usd_total",
                "Total estimated priced LLM token cost in USD.",
            ),
            llm_unpriced_tokens: counter_vec(
                "cordy_llm_unpriced_tokens_total",
                "Total LLM tokens for model aliases without a fixed TSR price.",
            ),
            llm_requests: counter_vec(
                "cordy_llm_request_total",
                "Total task usage reports by normalized LLM provider and model.",
            ),
            task_queued_expired: counter_vec(
                "cordy_task_queued_expired_total",
                "Total queued tasks expired by the scheduler.",
            ),
            task_lease_expired: counter_vec(
                "cordy_task_lease_expired_total",
                "Total dispatched or running task leases expired by the scheduler.",
            ),
            chat_claim_session_fallback_needed: Counter::new(
                "cordy_chat_claim_session_fallback_needed_total",
                "Total chat claims whose session pointer lacked a provider session or workdir.",
            )
            .expect("valid counter"),
            chat_claim_session_fallback_result: counter_vec(
                "cordy_chat_claim_session_fallback_result_total",
                "Total chat-claim session fallback query results (hit, miss, or error).",
            ),
            chat_claim_resume_query_duration: histogram_vec(
                "cordy_chat_claim_resume_query_duration_seconds",
                "Duration of chat-claim resume-history queries by fixed query name.",
                CHAT_CLAIM_RESUME_QUERY_DURATION_BUCKETS,
            ),
            runtime_gc_deleted: Counter::new(
                "cordy_runtime_gc_deleted_total",
                "Total stale offline runtimes safely deleted by garbage collection.",
            )
            .expect("valid counter"),
            runtime_gc_failed: Counter::new(
                "cordy_runtime_gc_failed_total",
                "Total runtime garbage-collection operations that failed.",
            )
            .expect("valid counter"),
            runtime_gc_blocked: Gauge::new(
                "cordy_runtime_gc_blocked_runtimes",
                "Bounded count of stale offline runtimes blocked from garbage collection by non-terminal tasks.",
            )
            .expect("valid gauge"),
            runtime_gc_blocked_observation_failed: Counter::new(
                "cordy_runtime_gc_blocked_observation_failed_total",
                "Total failures while observing stale runtimes blocked from garbage collection.",
            )
            .expect("valid counter"),
            entitlement_config_error: Counter::new(
                "cordy_entitlement_config_error_total",
                "Total startup failures caused by explicitly enabled but invalid entitlement policy configuration.",
            )
            .expect("valid counter"),
            entitlement_cache: counter_vec(
                "cordy_entitlement_cache_total",
                "Total entitlement cache outcomes.",
            ),
            entitlement_refresh: counter_vec(
                "cordy_entitlement_refresh_total",
                "Total entitlement refresh outcomes.",
            ),
            entitlement_refresh_duration: histogram_vec(
                "cordy_entitlement_refresh_duration_seconds",
                "Duration of entitlement refreshes.",
                CHAT_CLAIM_RESUME_QUERY_DURATION_BUCKETS,
            ),
            entitlement_decision: counter_vec(
                "cordy_entitlement_decision_total",
                "Total entitlement decisions by bounded gate, action, and reason.",
            ),
            entitlement_version_regression: counter_vec(
                "cordy_entitlement_version_regression_total",
                "Total rejected entitlement version regressions.",
            ),
            autopilot_quota_decision: counter_vec(
                "cordy_autopilot_quota_decision_total",
                "Total autopilot quota admission outcomes.",
            ),
            autopilot_failure_monitor: counter_vec(
                "cordy_autopilot_failure_monitor_total",
                "Total autopilot failure monitor outcomes by bounded stage.",
            ),
            active_tasks: Mutex::new(HashMap::new()),
            events: BusinessEventMetrics::new(),
        };
        m.prewarm_failure_reasons();
        m
    }

    /// Registers every collector (business + PR3 events) with `registry`,
    /// mirroring the Go Collectors() slice consumed by the /metrics endpoint.
    pub fn register_all(&self, registry: &prometheus::Registry) {
        let collectors: Vec<Box<dyn prometheus::core::Collector>> = vec![
            Box::new(self.task_enqueued.clone()),
            Box::new(self.task_dispatched.clone()),
            Box::new(self.task_started.clone()),
            Box::new(self.task_terminal.clone()),
            Box::new(self.task_failed.clone()),
            Box::new(self.task_queue_wait.clone()),
            Box::new(self.task_run_seconds.clone()),
            Box::new(self.task_total_seconds.clone()),
            Box::new(self.task_in_progress.clone()),
            Box::new(self.task_iterations.clone()),
            Box::new(self.llm_tokens.clone()),
            Box::new(self.llm_cost_usd.clone()),
            Box::new(self.llm_unpriced_tokens.clone()),
            Box::new(self.llm_requests.clone()),
            Box::new(self.task_queued_expired.clone()),
            Box::new(self.task_lease_expired.clone()),
            Box::new(self.chat_claim_session_fallback_needed.clone()),
            Box::new(self.chat_claim_session_fallback_result.clone()),
            Box::new(self.chat_claim_resume_query_duration.clone()),
            Box::new(self.runtime_gc_deleted.clone()),
            Box::new(self.runtime_gc_failed.clone()),
            Box::new(self.runtime_gc_blocked.clone()),
            Box::new(self.runtime_gc_blocked_observation_failed.clone()),
            Box::new(self.entitlement_config_error.clone()),
            Box::new(self.entitlement_cache.clone()),
            Box::new(self.entitlement_refresh.clone()),
            Box::new(self.entitlement_refresh_duration.clone()),
            Box::new(self.entitlement_decision.clone()),
            Box::new(self.entitlement_version_regression.clone()),
            Box::new(self.autopilot_quota_decision.clone()),
            Box::new(self.autopilot_failure_monitor.clone()),
        ];
        for c in collectors {
            registry.register(c).expect("unique collector");
        }
        for c in self.events.collectors() {
            registry.register(c).expect("unique collector");
        }
    }

    pub fn record_entitlement_config_error(&self) {
        self.entitlement_config_error.inc();
    }

    pub fn record_entitlement_cache(&self, outcome: &str) {
        self.entitlement_cache.with_label_values(&[outcome]).inc();
    }

    pub fn record_entitlement_refresh(&self, outcome: &str, seconds: f64) {
        self.entitlement_refresh.with_label_values(&[outcome]).inc();
        self.entitlement_refresh_duration
            .with_label_values(&[outcome])
            .observe(seconds);
    }

    pub fn record_entitlement_decision(&self, gate: &str, action: &str, reason: &str) {
        self.entitlement_decision
            .with_label_values(&[gate, action, reason])
            .inc();
    }

    pub fn record_entitlement_version_regression(&self, source: &str) {
        self.entitlement_version_regression
            .with_label_values(&[source])
            .inc();
    }

    pub fn record_autopilot_quota_decision(&self, action: &str, source: &str, result: &str) {
        let source = match source {
            "schedule" | "webhook" | "manual" | "api" => source,
            _ => "other",
        };
        self.autopilot_quota_decision
            .with_label_values(&[action, source, result])
            .inc();
    }

    pub fn record_autopilot_failure_monitor(&self, action: &str, outcome: &str) {
        let action = match action {
            "sweep" | "candidate" | "pause" | "rule_version" | "recipient" | "inbox"
            | "shutdown" => action,
            _ => "candidate",
        };
        let outcome = match outcome {
            "success" | "retryable_error" | "permanent_error" | "already_inactive"
            | "no_recipient" | "cancelled" | "timed_out" => outcome,
            _ => "permanent_error",
        };
        self.autopilot_failure_monitor
            .with_label_values(&[action, outcome])
            .inc();
    }

    pub fn record_runtime_gc_deleted(&self) {
        self.runtime_gc_deleted.inc();
    }

    pub fn record_runtime_gc_failed(&self) {
        self.runtime_gc_failed.inc();
    }

    pub fn set_runtime_gc_blocked(&self, count: i64) {
        self.runtime_gc_blocked.set(count as f64);
    }

    pub fn record_runtime_gc_blocked_observation_failed(&self) {
        self.runtime_gc_blocked_observation_failed.inc();
    }

    pub fn record_task_enqueued(&self, source: &str, runtime_mode: &str) {
        self.task_enqueued
            .with_label_values(&[
                &normalize_task_source(source),
                &normalize_runtime_mode(runtime_mode),
            ])
            .inc();
    }

    pub fn record_task_dispatched(
        &self,
        task_id: &str,
        source: &str,
        runtime_mode: &str,
        queue_wait_seconds: f64,
    ) {
        let source = normalize_task_source(source);
        let runtime_mode = normalize_runtime_mode(runtime_mode);
        self.task_dispatched
            .with_label_values(&[&source, &runtime_mode])
            .inc();
        if queue_wait_seconds >= 0.0 {
            self.task_queue_wait
                .with_label_values(&[&source, &runtime_mode])
                .observe(queue_wait_seconds);
        }
        self.mark_task_in_progress(task_id, &source, &runtime_mode);
    }

    pub fn record_task_started(&self, source: &str, runtime_mode: &str, provider: &str) {
        self.task_started
            .with_label_values(&[
                &normalize_task_source(source),
                &normalize_runtime_mode(runtime_mode),
                &normalize_runtime_provider(provider),
            ])
            .inc();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_task_terminal(
        &self,
        task_id: &str,
        source: &str,
        runtime_mode: &str,
        terminal_status: &str,
        run_seconds: f64,
        total_seconds: f64,
        attempt: i32,
    ) {
        let source = normalize_task_source(source);
        let runtime_mode = normalize_runtime_mode(runtime_mode);
        let terminal_status = normalize_terminal_status(terminal_status);
        self.task_terminal
            .with_label_values(&[&source, &runtime_mode, &terminal_status])
            .inc();
        if run_seconds >= 0.0 {
            self.task_run_seconds
                .with_label_values(&[&source, &runtime_mode, &terminal_status])
                .observe(run_seconds);
        }
        if total_seconds >= 0.0 {
            self.task_total_seconds
                .with_label_values(&[&source, &runtime_mode, &terminal_status])
                .observe(total_seconds);
        }
        let attempt = attempt.max(1);
        self.task_iterations
            .with_label_values(&[&source, &terminal_status])
            .observe(attempt as f64);
        self.clear_task_in_progress(task_id);
    }

    pub fn record_task_failed(&self, source: &str, runtime_mode: &str, failure_reason: &str) {
        self.task_failed
            .with_label_values(&[
                &normalize_task_source(source),
                &normalize_runtime_mode(runtime_mode),
                &normalize_failure_reason(failure_reason),
            ])
            .inc();
    }

    pub fn record_task_queued_expired(&self, source: &str, runtime_mode: &str) {
        self.task_queued_expired
            .with_label_values(&[
                &normalize_task_source(source),
                &normalize_runtime_mode(runtime_mode),
            ])
            .inc();
    }

    pub fn record_task_lease_expired(&self, source: &str) {
        self.task_lease_expired
            .with_label_values(&[&normalize_task_source(source)])
            .inc();
    }

    /// Counts a claim whose chat-session pointer lacked either the provider
    /// session or the workdir.
    pub fn record_chat_claim_session_fallback_needed(&self) {
        self.chat_claim_session_fallback_needed.inc();
    }

    pub fn record_chat_claim_session_fallback_hit(&self) {
        self.record_chat_claim_session_fallback_result_value("hit");
    }

    pub fn record_chat_claim_session_fallback_miss(&self) {
        self.record_chat_claim_session_fallback_result_value("miss");
    }

    pub fn record_chat_claim_session_fallback_error(&self) {
        self.record_chat_claim_session_fallback_result_value("error");
    }

    fn record_chat_claim_session_fallback_result_value(&self, result: &str) {
        self.chat_claim_session_fallback_result
            .with_label_values(&[result])
            .inc();
    }

    fn observe_chat_claim_resume_query(&self, query: &str, seconds: f64) {
        if seconds < 0.0 {
            return;
        }
        self.chat_claim_resume_query_duration
            .with_label_values(&[query])
            .observe(seconds);
    }

    pub fn observe_chat_claim_last_session_query(&self, seconds: f64) {
        self.observe_chat_claim_resume_query("last_session", seconds);
    }

    pub fn observe_chat_claim_rollout_missing_query(&self, seconds: f64) {
        self.observe_chat_claim_resume_query("rollout_missing", seconds);
    }

    /// Records LLM token usage and estimated cost. When the provider reported
    /// its own price (`cost_usd_ticks`, in 1e-10 USD units) it wins over the
    /// rate table: the table cannot express request-level rules such as xAI's
    /// 2x surcharge above a 200K prompt.
    #[allow(clippy::too_many_arguments)]
    pub fn record_llm_usage(
        &self,
        source: &str,
        runtime_mode: &str,
        raw_provider: &str,
        model_alias: &str,
        input_tokens: i64,
        output_tokens: i64,
        cache_read_tokens: i64,
        cache_write_tokens: i64,
        cost_usd_ticks: i64,
    ) {
        let source = normalize_task_source(source);
        let runtime_mode = normalize_runtime_mode(runtime_mode);
        let Some(price) = price_for_model_alias(model_alias) else {
            let provider = normalize_runtime_provider(raw_provider);
            let alias = crate::labels::normalize_model_alias(model_alias);
            self.record_unpriced_tokens(&provider, &alias, "input", input_tokens);
            self.record_unpriced_tokens(&provider, &alias, "output", output_tokens);
            self.record_unpriced_tokens(&provider, &alias, "cache_read", cache_read_tokens);
            self.record_unpriced_tokens(&provider, &alias, "cache_write", cache_write_tokens);
            // Having no rate row does not mean having no cost: the provider may
            // have priced the turn itself. Without rates there is nothing to
            // split the total by, so it lands whole in the `input` bucket.
            if cost_usd_ticks > 0 {
                self.llm_cost_usd
                    .with_label_values(&[
                        &provider,
                        &alias,
                        &normalize_token_type("input"),
                        &runtime_mode,
                        &source,
                    ])
                    .inc_by(cost_usd_ticks as f64 / COST_USD_TICKS_PER_USD as f64);
            }
            self.llm_requests
                .with_label_values(&[&provider, "unknown", &runtime_mode])
                .inc();
            return;
        };

        let mut costs = [
            token_cost_usd(input_tokens, price.input_per_m),
            token_cost_usd(output_tokens, price.output_per_m),
            token_cost_usd(cache_read_tokens, price.cache_read_per_m),
            token_cost_usd(cache_write_tokens, price.cache_write_per_m),
        ];
        if cost_usd_ticks > 0 {
            costs = distribute_authoritative_cost(
                cost_usd_ticks as f64 / COST_USD_TICKS_PER_USD as f64,
                costs,
            );
        }

        self.record_priced_tokens(
            price.provider,
            price.model,
            "input",
            &runtime_mode,
            &source,
            input_tokens,
            costs[0],
        );
        self.record_priced_tokens(
            price.provider,
            price.model,
            "output",
            &runtime_mode,
            &source,
            output_tokens,
            costs[1],
        );
        self.record_priced_tokens(
            price.provider,
            price.model,
            "cache_read",
            &runtime_mode,
            &source,
            cache_read_tokens,
            costs[2],
        );
        self.record_priced_tokens(
            price.provider,
            price.model,
            "cache_write",
            &runtime_mode,
            &source,
            cache_write_tokens,
            costs[3],
        );
        self.llm_requests
            .with_label_values(&[price.provider, price.model, &runtime_mode])
            .inc();
    }

    #[allow(clippy::too_many_arguments)]
    fn record_priced_tokens(
        &self,
        provider: &str,
        model: &str,
        token_type: &str,
        runtime_mode: &str,
        source: &str,
        tokens: i64,
        cost: f64,
    ) {
        if tokens <= 0 {
            return;
        }
        let token_type = normalize_token_type(token_type);
        self.llm_tokens
            .with_label_values(&[provider, model, &token_type, runtime_mode, source])
            .inc_by(tokens as f64);
        if cost > 0.0 {
            self.llm_cost_usd
                .with_label_values(&[provider, model, &token_type, runtime_mode, source])
                .inc_by(cost);
        }
    }

    fn record_unpriced_tokens(
        &self,
        provider: &str,
        model_alias: &str,
        token_type: &str,
        tokens: i64,
    ) {
        if tokens <= 0 {
            return;
        }
        self.llm_unpriced_tokens
            .with_label_values(&[provider, model_alias, &normalize_token_type(token_type)])
            .inc_by(tokens as f64);
    }

    fn mark_task_in_progress(&self, task_id: &str, source: &str, runtime_mode: &str) {
        if task_id.is_empty() {
            self.task_in_progress
                .with_label_values(&[source, runtime_mode])
                .inc();
            return;
        }
        let mut active = self.active_tasks.lock().unwrap_or_else(|e| e.into_inner());
        if active.contains_key(task_id) {
            return;
        }
        active.insert(
            task_id.to_string(),
            ActiveTaskLabels {
                source: source.to_string(),
                runtime_mode: runtime_mode.to_string(),
            },
        );
        self.task_in_progress
            .with_label_values(&[source, runtime_mode])
            .inc();
    }

    fn clear_task_in_progress(&self, task_id: &str) {
        if task_id.is_empty() {
            return;
        }
        let removed = self
            .active_tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(task_id);
        if let Some(labels) = removed {
            self.task_in_progress
                .with_label_values(&[&labels.source, &labels.runtime_mode])
                .dec();
        }
    }

    fn prewarm_failure_reasons(&self) {
        static SOURCES: LazyLock<Vec<String>> = LazyLock::new(|| {
            [
                "issue",
                "chat",
                "autopilot",
                "autopilot_issue",
                "quick_create",
                "other",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect()
        });
        static MODES: LazyLock<Vec<String>> = LazyLock::new(|| {
            ["local", "cloud", "unknown"]
                .iter()
                .map(|s| s.to_string())
                .collect()
        });
        for source in SOURCES.iter() {
            for mode in MODES.iter() {
                for reason in cordy_task_failure::all_reasons() {
                    self.task_failed
                        .with_label_values(&[source, mode, reason.as_str()])
                        .inc_by(0.0);
                }
            }
        }
    }
}

/// Rescales the per-token-type estimates so they sum to the provider's actual
/// charge. Only the total is authoritative — the per-type split remains an
/// estimate scaled from the rate table's proportions. A zero estimate has no
/// proportions to scale, so the charge lands on `input` to avoid dropping real
/// spend from the total.
fn distribute_authoritative_cost(actual: f64, mut estimated: [f64; 4]) -> [f64; 4] {
    let total = estimated[0] + estimated[1] + estimated[2] + estimated[3];
    if total <= 0.0 {
        return [actual, 0.0, 0.0, 0.0];
    }
    let scale = actual / total;
    for e in &mut estimated {
        *e *= scale;
    }
    estimated
}

impl Default for BusinessMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatched_marks_in_progress_and_terminal_clears_it() {
        let m = BusinessMetrics::new();
        let gauge = |m: &BusinessMetrics| {
            m.task_in_progress
                .with_label_values(&["issue", "local"])
                .get()
        };
        assert_eq!(gauge(&m), 0.0);
        m.record_task_dispatched("t1", "issue", "local", 1.5);
        assert_eq!(gauge(&m), 1.0);
        // Duplicate dispatch of the same task must not double-count.
        m.record_task_dispatched("t1", "issue", "local", 2.0);
        assert_eq!(gauge(&m), 1.0);

        m.record_task_terminal("t1", "issue", "local", "completed", 30.0, 31.5, 2);
        assert_eq!(gauge(&m), 0.0);
        // Clearing again is a no-op.
        m.record_task_terminal("t1", "issue", "local", "completed", 30.0, 31.5, 2);
        assert_eq!(gauge(&m), 0.0);
    }

    #[test]
    fn empty_task_id_bypasses_dedup_map() {
        let m = BusinessMetrics::new();
        m.record_task_dispatched("", "chat", "cloud", -1.0);
        assert_eq!(
            m.task_in_progress
                .with_label_values(&["chat", "cloud"])
                .get(),
            1.0
        );
        // Terminal with empty id cannot clear it (Go semantics).
        m.record_task_terminal("", "chat", "cloud", "failed", -1.0, -1.0, 1);
        assert_eq!(
            m.task_in_progress
                .with_label_values(&["chat", "cloud"])
                .get(),
            1.0
        );
    }

    #[test]
    fn attempt_clamped_to_one_for_iterations_histogram() {
        let m = BusinessMetrics::new();
        m.record_task_terminal("t", "api", "local", "cancelled", -1.0, -1.0, 0);
        let h = m.task_iterations.with_label_values(&["api", "cancelled"]);
        assert_eq!(h.get_sample_count(), 1);
    }

    #[test]
    fn llm_usage_unpriced_lands_whole_cost_on_input() {
        let m = BusinessMetrics::new();
        m.record_llm_usage(
            "issue",
            "local",
            "grok",
            "grok-composer-9",
            100,
            50,
            10,
            5,
            2_000_000_000,
        );
        let unpriced = |ty: &str| {
            m.llm_unpriced_tokens
                .with_label_values(&["grok", "grok-composer-9", ty])
                .get()
        };
        assert_eq!(unpriced("input"), 100.0);
        assert_eq!(unpriced("output"), 50.0);
        assert_eq!(unpriced("cache_read"), 10.0);
        assert_eq!(unpriced("cache_write"), 5.0);
        // Provider-reported cost kept whole under the input bucket.
        assert_eq!(
            m.llm_cost_usd
                .with_label_values(&["grok", "grok-composer-9", "input", "local", "issue"])
                .get(),
            0.2
        );
        // Requests bucket uses "unknown" model for unpriced aliases.
        assert_eq!(
            m.llm_requests
                .with_label_values(&["grok", "unknown", "local"])
                .get(),
            1.0
        );
    }

    #[test]
    fn llm_usage_priced_splits_by_rate_and_rescales_to_authoritative_total() {
        let m = BusinessMetrics::new();
        // luna: input $1/M, output $6/M, cache read $0.10/M, write $1.25/M.
        m.record_llm_usage(
            "chat",
            "cloud",
            "codex",
            "gpt-5.6-luna",
            1_000_000,
            1_000_000,
            0,
            0,
            0,
        );
        let tokens = |ty: &str| {
            m.llm_tokens
                .with_label_values(&["openai", "gpt-5.6-luna", ty, "cloud", "chat"])
                .get()
        };
        assert_eq!(tokens("input"), 1_000_000.0);
        assert_eq!(tokens("output"), 1_000_000.0);
        let cost = |ty: &str| {
            m.llm_cost_usd
                .with_label_values(&["openai", "gpt-5.6-luna", ty, "cloud", "chat"])
                .get()
        };
        assert!((cost("input") - 1.0).abs() < 1e-9);
        assert!((cost("output") - 6.0).abs() < 1e-9);

        // With an authoritative total of $7 the split rescales proportionally
        // (1:6) while keeping the sum exact.
        m.record_llm_usage(
            "chat",
            "cloud",
            "codex",
            "gpt-5.6-luna",
            1_000_000,
            1_000_000,
            0,
            0,
            70_000_000_000,
        );
        assert!((cost("input") - 1.0 - 1.0).abs() < 1e-9);
        assert!((cost("output") - 6.0 - 6.0).abs() < 1e-9);
        assert_eq!(
            m.llm_requests
                .with_label_values(&["openai", "gpt-5.6-luna", "cloud"])
                .get(),
            2.0
        );
    }

    #[test]
    fn distribute_zero_estimate_puts_everything_on_input() {
        let out = super::distribute_authoritative_cost(0.75, [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(out, [0.75, 0.0, 0.0, 0.0]);

        let out = super::distribute_authoritative_cost(7.0, [1.0, 6.0, 0.0, 0.0]);
        assert!((out[0] - 1.0).abs() < 1e-9 && (out[1] - 6.0).abs() < 1e-9);
        assert!((out.iter().sum::<f64>() - 7.0).abs() < 1e-9);
    }

    #[test]
    fn prewarm_creates_full_failure_reason_grid() {
        let m = BusinessMetrics::new();
        let registry = prometheus::Registry::new();
        m.register_all(&registry);
        let families = registry.gather();
        let failed = families
            .iter()
            .find(|f| f.name() == "cordy_agent_task_failed_total")
            .expect("task_failed family exported");
        // 6 sources × 3 modes × 25 reasons.
        assert_eq!(
            failed.get_metric().len(),
            6 * 3 * cordy_task_failure::all_reasons().len()
        );
    }

    #[test]
    fn autopilot_quota_source_allowlist() {
        let m = BusinessMetrics::new();
        m.record_autopilot_quota_decision("admit", "schedule", "allowed");
        m.record_autopilot_quota_decision("admit", "bogus", "allowed");
        assert_eq!(
            m.autopilot_quota_decision
                .with_label_values(&["admit", "schedule", "allowed"])
                .get(),
            1.0
        );
        assert_eq!(
            m.autopilot_quota_decision
                .with_label_values(&["admit", "other", "allowed"])
                .get(),
            1.0
        );
    }

    #[test]
    fn chat_claim_fallback_and_gc_counters() {
        let m = BusinessMetrics::new();
        m.record_chat_claim_session_fallback_needed();
        m.record_chat_claim_session_fallback_hit();
        m.record_chat_claim_session_fallback_miss();
        m.record_chat_claim_session_fallback_error();
        assert_eq!(m.chat_claim_session_fallback_needed.get(), 1.0);
        assert_eq!(
            m.chat_claim_session_fallback_result
                .with_label_values(&["hit"])
                .get(),
            1.0
        );
        // Negative durations ignored.
        m.observe_chat_claim_last_session_query(-1.0);
        assert_eq!(
            m.chat_claim_resume_query_duration
                .with_label_values(&["last_session"])
                .get_sample_count(),
            0
        );
        m.observe_chat_claim_last_session_query(0.02);
        assert_eq!(
            m.chat_claim_resume_query_duration
                .with_label_values(&["last_session"])
                .get_sample_count(),
            1
        );

        m.set_runtime_gc_blocked(7);
        assert_eq!(m.runtime_gc_blocked.get(), 7.0);
        m.record_runtime_gc_deleted();
        assert_eq!(m.runtime_gc_deleted.get(), 1.0);
    }
}
