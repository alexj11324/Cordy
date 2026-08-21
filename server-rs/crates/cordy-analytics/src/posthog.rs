//! Live PostHog client — port of `server/internal/analytics/posthog.go`.
//!
//! Ships events to PostHog's /batch/ endpoint: enqueues into a bounded buffer
//! (non-blocking capture) and flushes them from a background worker.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::client::{AnalyticsClient, Event, Props};
use crate::events::EVENT_SCHEMA_VERSION;

const DEFAULT_QUEUE_SIZE: usize = 1024;
const DEFAULT_BATCH_SIZE: usize = 64;
const DEFAULT_FLUSH_EVERY: Duration = Duration::from_secs(10);
const DEFAULT_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

/// Configures the live PostHog client. Zero-value optional fields fall back to
/// sensible defaults.
#[derive(Debug, Clone)]
pub struct PostHogConfig {
    pub api_key: String,
    pub host: String,
    pub environment: String,
    pub queue_size: usize,
    pub batch_size: usize,
    pub flush_every: Duration,
}

impl Default for PostHogConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            host: crate::client::DEFAULT_POSTHOG_HOST.to_string(),
            environment: String::new(),
            queue_size: DEFAULT_QUEUE_SIZE,
            batch_size: DEFAULT_BATCH_SIZE,
            flush_every: DEFAULT_FLUSH_EVERY,
        }
    }
}

struct Inner {
    cfg: PostHogConfig,
    tx: mpsc::Sender<Event>,
    cancel: CancellationToken,
    handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    dropped: AtomicU64,
    sent: AtomicU64,
    failed: AtomicU64,
    http: reqwest::Client,
}

/// Bounded-buffer PostHog shipper. [`AnalyticsClient::capture`] returns
/// immediately; a background worker batches and flushes.
#[derive(Clone)]
pub struct PostHogClient {
    inner: Arc<Inner>,
}

impl PostHogClient {
    /// Starts the background flush worker. The caller must [`AnalyticsClient::close`]
    /// on shutdown to drain pending events.
    pub fn new(mut cfg: PostHogConfig) -> Self {
        if cfg.queue_size == 0 {
            cfg.queue_size = DEFAULT_QUEUE_SIZE;
        }
        if cfg.batch_size == 0 {
            cfg.batch_size = DEFAULT_BATCH_SIZE;
        }
        if cfg.flush_every.is_zero() {
            cfg.flush_every = DEFAULT_FLUSH_EVERY;
        }
        if cfg.environment.is_empty() {
            cfg.environment = crate::client::environment_from_env();
        }
        let (tx, rx) = mpsc::channel(cfg.queue_size);
        let http = reqwest::Client::builder()
            .timeout(DEFAULT_FLUSH_TIMEOUT)
            .build()
            .expect("reqwest client");
        let inner = Arc::new(Inner {
            cfg,
            tx,
            cancel: CancellationToken::new(),
            handle: Mutex::new(None),
            dropped: AtomicU64::new(0),
            sent: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            http,
        });
        let handle = tokio::spawn(run(inner.clone(), rx));
        *inner.handle.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
        Self { inner }
    }

    pub fn sent(&self) -> u64 {
        self.inner.sent.load(Ordering::Relaxed)
    }

    pub fn dropped(&self) -> u64 {
        self.inner.dropped.load(Ordering::Relaxed)
    }

    pub fn failed(&self) -> u64 {
        self.inner.failed.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl AnalyticsClient for PostHogClient {
    /// Enqueues an event. Returns immediately; on a full queue the event is
    /// dropped and counted. Analytics must never block a request handler.
    fn capture(&self, mut event: Event) {
        if event.timestamp.is_none() {
            event.timestamp = Some(Utc::now());
        }
        match self.inner.tx.try_send(event) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                let n = self.inner.dropped.fetch_add(1, Ordering::Relaxed) + 1;
                // Log periodically — every 100 drops — so a broken pipe is
                // visible but doesn't spam logs under sustained load.
                if n % 100 == 1 {
                    tracing::warn!(total_dropped = n, "analytics: queue full, dropping event");
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.inner.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Stops accepting events and drains whatever is already queued.
    async fn close(&self) {
        self.inner.cancel.cancel();
        let handle = self
            .inner
            .handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(handle) = handle {
            let _ = handle.await;
        }
        tracing::info!(
            sent = self.sent(),
            dropped = self.dropped(),
            failed = self.failed(),
            "analytics: posthog client closed"
        );
    }
}

async fn run(inner: Arc<Inner>, mut rx: mpsc::Receiver<Event>) {
    let mut ticker = tokio::time::interval(inner.cfg.flush_every);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut batch: Vec<Event> = Vec::with_capacity(inner.cfg.batch_size);

    loop {
        tokio::select! {
            maybe = rx.recv() => match maybe {
                Some(event) => {
                    batch.push(event);
                    if batch.len() >= inner.cfg.batch_size {
                        send(&inner, &mut batch).await;
                    }
                }
                // All senders dropped: flush and exit.
                None => {
                    send(&inner, &mut batch).await;
                    return;
                }
            },
            _ = ticker.tick() => send(&inner, &mut batch).await,
            _ = inner.cancel.cancelled() => {
                // Drain remaining events. The channel is not closed by close()
                // to avoid racing with capture, so we loop until it's empty.
                loop {
                    match rx.try_recv() {
                        Ok(event) => {
                            batch.push(event);
                            if batch.len() >= inner.cfg.batch_size {
                                send(&inner, &mut batch).await;
                            }
                        }
                        Err(_) => {
                            send(&inner, &mut batch).await;
                            return;
                        }
                    }
                }
            }
        }
    }
}

/// Mirrors the PostHog /batch/ JSON shape.
#[derive(Debug, Serialize)]
struct CapturePayload {
    api_key: String,
    batch: Vec<CaptureItem>,
}

#[derive(Debug, Serialize)]
struct CaptureItem {
    event: String,
    distinct_id: String,
    properties: Props,
    timestamp: String,
}

/// Pure payload builder extracted from the Go send() path so the enrichment
/// rules are unit-testable without network I/O.
fn build_capture_items(batch: &[Event], environment: &str) -> Vec<CaptureItem> {
    batch
        .iter()
        .map(|e| {
            let mut props = e.properties.clone().unwrap_or_default();
            if !e.workspace_id.is_empty() {
                props.insert(
                    "workspace_id".to_string(),
                    Value::String(e.workspace_id.clone()),
                );
            }
            props.insert(
                "event_schema_version".to_string(),
                Value::from(EVENT_SCHEMA_VERSION),
            );
            props.insert(
                "environment".to_string(),
                Value::String(environment.to_string()),
            );
            props
                .entry("is_demo".to_string())
                .or_insert(Value::Bool(false));
            if !props.contains_key("user_id")
                && !e.distinct_id.is_empty()
                && !e.distinct_id.contains(':')
            {
                props.insert("user_id".to_string(), Value::String(e.distinct_id.clone()));
            }
            if let Some(set_once) = e.set_once.as_ref().filter(|m| !m.is_empty()) {
                props.insert("$set_once".to_string(), Value::Object(set_once.clone()));
            }
            if let Some(set) = e.set.as_ref().filter(|m| !m.is_empty()) {
                props.insert("$set".to_string(), Value::Object(set.clone()));
            }
            let timestamp = e
                .timestamp
                .unwrap_or_else(Utc::now)
                // Go formats RFC3339Nano (trailing zeros trimmed); AutoSi picks
                // the shortest exact 0/3/6/9-digit form, which PostHog parses
                // identically.
                .to_rfc3339_opts(SecondsFormat::AutoSi, true);
            CaptureItem {
                event: e.name.clone(),
                distinct_id: e.distinct_id.clone(),
                properties: props,
                timestamp,
            }
        })
        .collect()
}

async fn send(inner: &Arc<Inner>, batch: &mut Vec<Event>) {
    if batch.is_empty() {
        return;
    }
    let n = batch.len() as u64;
    let items = build_capture_items(batch, &inner.cfg.environment);
    let payload = CapturePayload {
        api_key: inner.cfg.api_key.clone(),
        batch: items,
    };
    let body = match serde_json::to_vec(&payload) {
        Ok(body) => body,
        Err(error) => {
            inner.failed.fetch_add(n, Ordering::Relaxed);
            tracing::error!(%error, "analytics: marshal batch");
            batch.clear();
            return;
        }
    };

    let url = format!("{}/batch/", inner.cfg.host);
    let result = inner
        .http
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await;
    match result {
        Ok(resp) if resp.status().as_u16() < 400 => {
            inner.sent.fetch_add(n, Ordering::Relaxed);
        }
        Ok(resp) => {
            inner.failed.fetch_add(n, Ordering::Relaxed);
            tracing::warn!(
                status = %resp.status(),
                events = n,
                "analytics: posthog rejected batch"
            );
        }
        Err(error) => {
            inner.failed.fetch_add(n, Ordering::Relaxed);
            tracing::warn!(%error, events = n, "analytics: send batch failed");
        }
    }
    batch.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(name: &str, distinct_id: &str) -> Event {
        Event {
            name: name.to_string(),
            distinct_id: distinct_id.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn capture_items_enrich_properties() {
        let mut e = event("signup", "user-1");
        e.workspace_id = "ws-9".to_string();
        let mut props = Props::new();
        props.insert("signup_source".to_string(), json!("x"));
        props.insert("is_demo".to_string(), json!(true));
        props.insert("user_id".to_string(), json!("explicit"));
        e.properties = Some(props);
        e.set_once = Some(Props::from_iter([("email".to_string(), json!("a@b.co"))]));
        e.set = Some(Props::new()); // empty → omitted

        let items = build_capture_items(std::slice::from_ref(&e), "production");
        assert_eq!(items.len(), 1);
        let p = &items[0].properties;
        assert_eq!(p["workspace_id"], json!("ws-9"));
        assert_eq!(p["event_schema_version"], json!(2));
        assert_eq!(p["environment"], json!("production"));
        assert_eq!(p["is_demo"], json!(true), "existing is_demo preserved");
        assert_eq!(
            p["user_id"],
            json!("explicit"),
            "present user_id not overwritten"
        );
        assert_eq!(p["$set_once"], json!({"email": "a@b.co"}));
        assert!(!p.contains_key("$set"), "empty set map omitted");
        assert_eq!(items[0].event, "signup");
        assert_eq!(items[0].distinct_id, "user-1");
    }

    #[test]
    fn capture_items_default_is_demo_and_user_id_from_distinct() {
        let items = build_capture_items(&[event("issue_created", "user-7")], "dev");
        let p = &items[0].properties;
        assert_eq!(p["is_demo"], json!(false), "is_demo always stamped");
        assert_eq!(p["user_id"], json!("user-7"));
    }

    #[test]
    fn capture_items_no_user_id_for_synthetic_distinct() {
        // A distinct id containing ":" is a synthetic scope key, not a user.
        let items = build_capture_items(&[event("runtime_registered", "workspace:ws-1")], "dev");
        assert!(!items[0].properties.contains_key("user_id"));
    }

    /// End-to-end queue→batch→flush behavior against an unreachable host:
    /// every captured event must land in `failed`, none in `sent`, and close()
    /// must drain the queue before returning.
    #[tokio::test]
    async fn close_drains_queue_and_counts_failures() {
        let client = PostHogClient::new(PostHogConfig {
            api_key: "key".to_string(),
            host: "http://127.0.0.1:1".to_string(),
            environment: "dev".to_string(),
            ..PostHogConfig::default()
        });
        for i in 0..5 {
            client.capture(event("signup", &format!("u{i}")));
        }
        client.close().await;
        assert_eq!(client.dropped(), 0, "queue had room; nothing dropped");
        assert_eq!(client.sent(), 0);
        assert_eq!(
            client.failed(),
            5,
            "all drained events attempted and failed"
        );

        // Captures after close are counted as dropped (worker gone).
        client.capture(event("signup", "late"));
        assert_eq!(client.dropped(), 1);
    }
}
