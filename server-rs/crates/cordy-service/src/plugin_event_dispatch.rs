//! Event-triggered hooks — port of `server/internal/service/plugin_event_dispatch.go`
//! and `plugin_event_bridge.go`.
//!
//! The rule this file exists to keep: an event hook NEVER blocks the host. The
//! event bus is synchronous — `Bus::publish` runs its listeners inline, on the
//! task of whatever request produced the event — so a listener that dialled a
//! third-party endpoint would put an outside server on the critical path of
//! creating an issue. Everything here therefore hands off to a bounded worker
//! pool and returns immediately.
//!
//! The same reasoning is why the agent execution path has no hook at all: a hook
//! that must run before or after every agent turn is a third party holding the
//! product's main loop open. Agents reach hooks in PR 4 by choosing to call one
//! as a tool, which is a call they can decline.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use cordy_db::models::PluginInstallation;
use cordy_events::Bus;
use cordy_plugincontract::{
    Hook, EVENT_COMMENT_CREATED, EVENT_ISSUE_CREATED, EVENT_ISSUE_STATUS_CHANGED,
    EVENT_ISSUE_UPDATED, EVENT_TASK_COMPLETED, EVENT_TASK_FAILED, EVENT_TASK_STARTED,
    TRIGGER_EVENT,
};
use cordy_protocol::{
    EVENT_COMMENT_CREATED as PROTOCOL_COMMENT_CREATED,
    EVENT_ISSUE_CREATED as PROTOCOL_ISSUE_CREATED, EVENT_ISSUE_UPDATED as PROTOCOL_ISSUE_UPDATED,
    EVENT_TASK_COMPLETED as PROTOCOL_TASK_COMPLETED, EVENT_TASK_FAILED as PROTOCOL_TASK_FAILED,
    EVENT_TASK_RUNNING as PROTOCOL_TASK_RUNNING,
};

use crate::feature_flags::{plugins_v1_enabled, FlagSource};
use crate::plugin::{
    hook_allows_trigger, hook_failure_status, parse_installation_manifest, parse_uuid_value,
    redact_hook_error, PluginService,
};
use crate::plugin_hook::{
    hook_breaker_open, invoke_hook, HookInvocation, HOOK_EVENT_ATTEMPTS, HOOK_EVENT_BACKOFF,
};
use crate::plugin_token::HookActor;

/// Queue depth. Full means events are arriving faster than endpoints can
/// answer; the overflow is dropped and counted rather than queued forever,
/// because an unbounded queue turns a slow plugin into a memory leak.
const DISPATCH_QUEUE_DEPTH: usize = 512;
const DISPATCH_WORKERS: usize = 4;
const DISPATCH_JOB_TIMEOUT: Duration = Duration::from_secs(2 * 60);

/// How long a hook call stays on record. This table is operational telemetry,
/// not history: it answers "why is this endpoint failing right now", and the
/// circuit breaker and rate limiter only ever look minutes back.
pub const INVOCATION_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const INVOCATION_SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);

struct DispatchJob {
    workspace_id: Uuid,
    event_type: String,
    payload: serde_json::Value,
}

/// Fans domain events out to the hooks that asked for them.
pub struct PluginEventDispatcher {
    service: Arc<PluginService>,
    callbacks: Arc<crate::plugin_token::CallbackTokens>,
    callback_base_url: String,
    feature_flags: Option<Arc<dyn FlagSource>>,
    queue: tokio::sync::mpsc::Sender<DispatchJob>,
    /// Tokio mutex so a worker can hold it across `recv()` without making the
    /// worker future non-Send; four workers claim jobs through it serially.
    queue_rx: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<DispatchJob>>,
    stop: CancellationToken,
    started: AtomicBool,
    /// Counts events shed under backpressure, surfaced for triage.
    dropped: AtomicI64,
}

impl PluginEventDispatcher {
    pub fn new(
        service: Arc<PluginService>,
        callbacks: Arc<crate::plugin_token::CallbackTokens>,
        callback_base_url: String,
        feature_flags: Option<Arc<dyn FlagSource>>,
    ) -> Self {
        let (queue_tx, queue_rx) = tokio::sync::mpsc::channel(DISPATCH_QUEUE_DEPTH);
        Self {
            service,
            callbacks,
            callback_base_url,
            feature_flags,
            queue: queue_tx,
            queue_rx: tokio::sync::Mutex::new(queue_rx),
            stop: CancellationToken::new(),
            started: AtomicBool::new(false),
            dropped: AtomicI64::new(0),
        }
    }

    /// Spawns the workers and the retention sweep. Call once after
    /// construction. Repeated calls are harmless and do not duplicate event
    /// deliveries.
    pub fn start(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        for _ in 0..DISPATCH_WORKERS {
            let dispatcher = Arc::clone(self);
            tokio::spawn(async move { dispatcher.work().await });
        }
        let dispatcher = Arc::clone(self);
        tokio::spawn(async move { dispatcher.sweep_invocations().await });
    }

    /// Makes the table's "TTL-swept" description true.
    ///
    /// Nothing reads a row older than the breaker and rate-limit windows, both
    /// of which look minutes back, so without this the table only grows — at up
    /// to the per-hook limit of 120 rows a minute, per hook, forever.
    ///
    /// The first sweep waits for the first tick rather than firing at
    /// construction (Go history: an immediate sweep panicked in cmd/server's
    /// router test over an unopened pool).
    async fn sweep_invocations(&self) {
        let first_sweep = tokio::time::Instant::now() + INVOCATION_SWEEP_EVERY;
        let mut ticker = tokio::time::interval_at(first_sweep, INVOCATION_SWEEP_EVERY);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = self.stop.cancelled() => return,
                _ = ticker.tick() => self.sweep_once().await,
            }
        }
    }

    /// Deletes what has aged out.
    async fn sweep_once(&self) {
        let cutoff = (chrono::Utc::now()
            - chrono::Duration::from_std(INVOCATION_RETENTION).unwrap_or_default())
        .to_string();
        if let Ok(cutoff) = cutoff.parse::<chrono::DateTime<chrono::Utc>>() {
            match cordy_db::queries::plugin::delete_expired_plugin_invocations(
                &self.service.pool,
                Some(cutoff),
            )
            .await
            {
                Ok(removed) if removed > 0 => {
                    tracing::info!(removed, "plugins: swept expired hook invocations");
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "plugins: invocation sweep failed"),
            }
        }
    }

    /// What the bus listener calls. It must return promptly: the caller is a
    /// live request that has already done its real work.
    ///
    /// Note it takes no context from that request. Tying an outbound hook to
    /// the request that triggered it would cancel the hook the moment the
    /// browser got its response, which is exactly when the hook is only just
    /// starting.
    pub fn dispatch(&self, event_type: &str, workspace_id: &str, payload: serde_json::Value) {
        if workspace_id.is_empty() {
            return;
        }
        let Ok(parsed_workspace) = parse_uuid_value(workspace_id) else {
            return;
        };

        // Nothing is inspected here. The installation lookup is a database read
        // and finding the issue id is a JSON round-trip; both belong on a
        // worker, and both sit behind the feature-flag check so a deployment
        // with plugins off pays for neither.
        match self.queue.try_send(DispatchJob {
            workspace_id: parsed_workspace,
            event_type: event_type.to_string(),
            payload,
        }) {
            Ok(()) => {}
            Err(_) => {
                let dropped = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
                tracing::warn!(
                    event_type,
                    dropped_total = dropped,
                    "plugins: event dispatch queue full, dropping event"
                );
            }
        }
    }

    /// Reports how many events were shed under backpressure.
    pub fn dropped(&self) -> i64 {
        self.dropped.load(Ordering::Relaxed)
    }

    async fn work(&self) {
        loop {
            // The tokio Mutex is held across recv() — fine, since holding it
            // just serializes which worker claims the next job; no lock spans
            // into job processing.
            let job = {
                let mut rx = self.queue_rx.lock().await;
                tokio::select! {
                    _ = self.stop.cancelled() => return,
                    job = rx.recv() => match job {
                        Some(job) => job,
                        None => return,
                    }
                }
            };
            let event_type = job.event_type.clone();
            match tokio::time::timeout(
                DISPATCH_JOB_TIMEOUT,
                AssertUnwindSafe(self.run(job)).catch_unwind(),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(recovered)) => {
                    tracing::error!(
                        event_type,
                        recovered = %panic_detail(recovered.as_ref()),
                        "plugins: panic while delivering an event hook"
                    );
                }
                Err(_) => {
                    tracing::warn!(
                        event_type,
                        timeout_seconds = DISPATCH_JOB_TIMEOUT.as_secs(),
                        "plugins: event hook delivery timed out"
                    );
                }
            }
        }
    }

    /// Resolves which hooks want this event and calls each one.
    async fn run(&self, mut job: DispatchJob) {
        // The flag gates this path too, and checking it HERE rather than at
        // subscription is the point: a deployment that turns plugins off after
        // something was installed must stop the outbound calls, not just hide
        // the UI. Reading it per delivery means the flip takes effect
        // immediately instead of at the next restart.
        //
        // It also keeps the flag-off cost at zero. Without this every
        // dispatched event ran a ListWorkspacePluginInstallations query to
        // discover there was nothing to call.
        if !plugin_events_enabled(self.feature_flags.as_deref()) {
            return;
        }

        // Only now, past the flag: the id is needed to narrow the callback
        // grant, and finding it means parsing the payload.
        let issue_id = issue_id_from_payload(&job.payload);

        let installations = match cordy_db::queries::plugin::list_workspace_plugin_installations(
            &self.service.pool,
            job.workspace_id,
        )
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(error = %e, "plugins: event dispatch could not list installations");
                return;
            }
        };
        for installation in &installations {
            if !installation.enabled {
                continue;
            }
            let manifest = match parse_installation_manifest(&crate::plugin::json_bytes(
                &installation.manifest,
            )) {
                Ok(manifest) => manifest,
                Err(_) => continue,
            };
            for hook in &manifest.contributes.hooks {
                if !hook_allows_trigger(hook, TRIGGER_EVENT)
                    || !hook_wants_event(hook, &job.event_type)
                {
                    continue;
                }
                self.deliver(installation, hook, &mut job, issue_id).await;
            }
        }
    }

    /// Runs one hook with the event retry schedule.
    async fn deliver(
        &self,
        installation: &PluginInstallation,
        hook: &Hook,
        job: &mut DispatchJob,
        issue_id: Option<Uuid>,
    ) {
        // A hook whose endpoint has been failing is not retried on every event.
        // Without this, an endpoint that has been down for an hour receives one
        // doomed request per workspace event, forever.
        if hook_breaker_open(&self.service.pool, installation.id, &hook.key).await {
            tracing::info!(hook = %hook.key, event_type = %job.event_type, "plugins: hook circuit open, skipping event");
            return;
        }

        // An event has no person behind it. Writes it produces are the
        // plugin's own, attributed to the installation.
        let actor = HookActor {
            actor_type: "plugin".to_string(),
            id: installation.id,
        };
        let payload = job.payload.clone();

        for attempt in 1..=HOOK_EVENT_ATTEMPTS {
            let invocation = HookInvocation {
                installation,
                hook,
                trigger: TRIGGER_EVENT,
                event_type: &job.event_type,
                actor: actor.clone(),
                issue_id,
                input: Some(&payload),
            };
            let (_, outcome) = invoke_hook(
                &self.service,
                Some(&self.callbacks),
                &self.callback_base_url,
                invocation,
                attempt,
            )
            .await;
            match outcome {
                Ok(()) => return,
                Err(err) => {
                    // A refusal is a decision, not an outage: retrying a hook
                    // that is disabled, out of scope or rate limited just burns
                    // the budget.
                    if hook_failure_status(&err) == "refused" {
                        tracing::info!(hook = %hook.key, error = %redact_hook_error(&err), "plugins: event hook refused");
                        return;
                    }
                    if attempt == HOOK_EVENT_ATTEMPTS {
                        tracing::warn!(hook = %hook.key, event_type = %job.event_type, error = %redact_hook_error(&err), "plugins: event hook failed after retries");
                        return;
                    }
                    tokio::select! {
                        _ = self.stop.cancelled() => return,
                        _ = tokio::time::sleep(HOOK_EVENT_BACKOFF * attempt as u32) => {}
                    }
                }
            }
        }
    }

    /// Stops the workers. Safe to call more than once.
    pub fn close(&self) {
        self.stop.cancel();
    }
}

fn hook_wants_event(hook: &Hook, event_type: &str) -> bool {
    hook.events.iter().any(|declared| declared == event_type)
}

fn plugin_events_enabled(flags: Option<&dyn FlagSource>) -> bool {
    flags.is_some_and(plugins_v1_enabled)
}

fn panic_detail(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic")
}

// ---------------------------------------------------------------------------
// Bridge — plugin_event_bridge.go
// ---------------------------------------------------------------------------

/// Wires the dispatcher onto the bus.
///
/// `Bus::publish` calls its listeners INLINE, on the task of the request that
/// published — so everything these closures do must be cheap and non-blocking.
/// They extract an id and hand off; the network call happens on a worker.
///
/// The listener does the least it possibly can: hand over the payload
/// unexamined. Extracting the issue id here would put a JSON round-trip of a
/// full issue body on the publishing request's task, for every one of these
/// events, in every workspace — including deployments where plugins are
/// switched off entirely. It happens on a worker instead, after the flag
/// check, where it costs nothing anyone is waiting for.
pub trait PluginEventSink: Send + Sync {
    fn dispatch(&self, event_type: &str, workspace_id: &str, payload: serde_json::Value);
}

impl PluginEventSink for PluginEventDispatcher {
    fn dispatch(&self, event_type: &str, workspace_id: &str, payload: serde_json::Value) {
        PluginEventDispatcher::dispatch(self, event_type, workspace_id, payload);
    }
}

pub fn subscribe_plugin_events<S>(bus: &Bus, dispatcher: Arc<S>)
where
    S: PluginEventSink + ?Sized + 'static,
{
    let forward = |plugin_event: &'static str| {
        let dispatcher = Arc::clone(&dispatcher);
        move |e: &cordy_events::Event| {
            dispatcher.dispatch(plugin_event, &e.workspace_id, e.payload.clone());
        }
    };

    bus.subscribe(PROTOCOL_ISSUE_CREATED, forward(EVENT_ISSUE_CREATED));
    bus.subscribe(PROTOCOL_COMMENT_CREATED, forward(EVENT_COMMENT_CREATED));
    bus.subscribe(PROTOCOL_TASK_RUNNING, forward(EVENT_TASK_STARTED));
    bus.subscribe(PROTOCOL_TASK_COMPLETED, forward(EVENT_TASK_COMPLETED));
    bus.subscribe(PROTOCOL_TASK_FAILED, forward(EVENT_TASK_FAILED));

    // issue.status_changed has no event of its own internally: a status change
    // is an issue:updated carrying status_changed=true. Deriving it here rather
    // than adding a second publish keeps one write producing one internal
    // event, and lets a plugin subscribe to the specific thing it cares about
    // instead of filtering every field change itself.
    let status_dispatcher = Arc::clone(&dispatcher);
    bus.subscribe(PROTOCOL_ISSUE_UPDATED, move |e: &cordy_events::Event| {
        status_dispatcher.dispatch(EVENT_ISSUE_UPDATED, &e.workspace_id, e.payload.clone());
        // A map lookup, not a parse: cheap enough for the request task.
        if payload_flag(&e.payload, "status_changed") {
            status_dispatcher.dispatch(
                EVENT_ISSUE_STATUS_CHANGED,
                &e.workspace_id,
                e.payload.clone(),
            );
        }
    });
}

/// Finds the issue an event is about, so the callback token issued for the hook
/// can be narrowed to it. A payload with no issue yields None, which simply
/// means the grant is not issue-scoped.
pub(crate) fn issue_id_from_payload(payload: &serde_json::Value) -> Option<Uuid> {
    let shape = payload.as_object()?;
    // Three shapes, in Go's order: {"issue":{"id":…}}, {"comment":{"issue_id":…}},
    // {"issue_id":…}.
    let candidates = [
        shape
            .get("issue")
            .and_then(|issue| issue.get("id"))
            .and_then(|v| v.as_str()),
        shape
            .get("comment")
            .and_then(|comment| comment.get("issue_id"))
            .and_then(|v| v.as_str()),
        shape.get("issue_id").and_then(|v| v.as_str()),
    ];
    for candidate in candidates.into_iter().flatten() {
        if candidate.is_empty() {
            continue;
        }
        if let Ok(parsed) = parse_uuid_value(candidate) {
            return Some(parsed);
        }
    }
    None
}

fn payload_flag(payload: &serde_json::Value, key: &str) -> bool {
    payload.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingSink {
        deliveries: Mutex<Vec<(String, String, serde_json::Value)>>,
    }

    impl PluginEventSink for RecordingSink {
        fn dispatch(&self, event_type: &str, workspace_id: &str, payload: serde_json::Value) {
            self.deliveries.lock().unwrap().push((
                event_type.to_string(),
                workspace_id.to_string(),
                payload,
            ));
        }
    }

    struct FixedFlags(bool);

    impl FlagSource for FixedFlags {
        fn is_enabled(&self, _key: &str, _default: bool) -> bool {
            self.0
        }
    }

    #[test]
    fn bridge_maps_the_complete_plugin_event_contract() {
        let bus = Bus::new();
        let sink = Arc::new(RecordingSink::default());
        subscribe_plugin_events(&bus, sink.clone());
        let workspace_id = "11111111-1111-4111-8111-111111111111";

        for (internal, payload) in [
            (PROTOCOL_ISSUE_CREATED, json!({"sequence": 1})),
            (PROTOCOL_COMMENT_CREATED, json!({"sequence": 2})),
            (PROTOCOL_TASK_RUNNING, json!({"sequence": 3})),
            (PROTOCOL_TASK_COMPLETED, json!({"sequence": 4})),
            (PROTOCOL_TASK_FAILED, json!({"sequence": 5})),
            (
                PROTOCOL_ISSUE_UPDATED,
                json!({"sequence": 6, "status_changed": false}),
            ),
            (
                PROTOCOL_ISSUE_UPDATED,
                json!({"sequence": 7, "status_changed": true}),
            ),
        ] {
            bus.publish(&cordy_events::Event {
                event_type: internal.to_string(),
                workspace_id: workspace_id.to_string(),
                payload,
                ..Default::default()
            });
        }

        let deliveries = sink.deliveries.lock().unwrap();
        assert_eq!(
            deliveries
                .iter()
                .map(|(event_type, _, _)| event_type.as_str())
                .collect::<Vec<_>>(),
            vec![
                EVENT_ISSUE_CREATED,
                EVENT_COMMENT_CREATED,
                EVENT_TASK_STARTED,
                EVENT_TASK_COMPLETED,
                EVENT_TASK_FAILED,
                EVENT_ISSUE_UPDATED,
                EVENT_ISSUE_UPDATED,
                EVENT_ISSUE_STATUS_CHANGED,
            ]
        );
        assert!(deliveries
            .iter()
            .all(|(_, delivered_workspace, _)| delivered_workspace == workspace_id));
        assert_eq!(deliveries.last().unwrap().2["sequence"], 7);
    }

    #[test]
    fn plugin_event_gate_fails_closed_without_a_source() {
        assert!(!plugin_events_enabled(None));
        assert!(!plugin_events_enabled(Some(&FixedFlags(false))));
        assert!(plugin_events_enabled(Some(&FixedFlags(true))));
    }

    #[test]
    fn issue_id_from_payload_reads_the_three_documented_shapes() {
        assert_eq!(
            issue_id_from_payload(
                &json!({"issue": {"id": "11111111-1111-4111-8111-111111111111"}})
            ),
            parse_uuid_value("11111111-1111-4111-8111-111111111111").ok()
        );
        assert_eq!(
            issue_id_from_payload(
                &json!({"comment": {"issue_id": "22222222-2222-4222-8222-222222222222"}})
            ),
            parse_uuid_value("22222222-2222-4222-8222-222222222222").ok()
        );
        assert_eq!(
            issue_id_from_payload(&json!({"issue_id": "33333333-3333-4333-8333-333333333333"})),
            parse_uuid_value("33333333-3333-4333-8333-333333333333").ok()
        );
    }

    #[test]
    fn payloads_without_an_issue_yield_no_grant_scope() {
        assert_eq!(issue_id_from_payload(&json!({"task": {"id": "x"}})), None);
        assert_eq!(issue_id_from_payload(&json!({})), None);
        assert_eq!(issue_id_from_payload(&serde_json::Value::Null), None);
        // A malformed id falls through to the next candidate rather than erroring.
        assert_eq!(
            issue_id_from_payload(
                &json!({"issue": {"id": "not-a-uuid"}, "issue_id": "44444444-4444-4444-8444-444444444444"})
            ),
            parse_uuid_value("44444444-4444-4444-8444-444444444444").ok()
        );
    }

    #[test]
    fn payload_flag_is_a_map_lookup_not_a_parse() {
        assert!(payload_flag(
            &json!({"status_changed": true}),
            "status_changed"
        ));
        assert!(!payload_flag(
            &json!({"status_changed": false}),
            "status_changed"
        ));
        assert!(!payload_flag(&json!({}), "status_changed"));
        assert!(!payload_flag(&serde_json::Value::Null, "status_changed"));
    }

    #[test]
    fn hook_wants_event_matches_declared_only() {
        let hook = Hook {
            events: vec![EVENT_ISSUE_CREATED.to_string()],
            ..empty_hook()
        };
        assert!(hook_wants_event(&hook, EVENT_ISSUE_CREATED));
        assert!(!hook_wants_event(&hook, EVENT_ISSUE_UPDATED));
    }

    fn empty_hook() -> Hook {
        Hook {
            key: String::new(),
            name: String::new(),
            description: String::new(),
            input_schema: None,
            triggers: Vec::new(),
            events: Vec::new(),
            transport: cordy_plugincontract::HookTransport::default(),
            timeout_ms: 0,
        }
    }
}
