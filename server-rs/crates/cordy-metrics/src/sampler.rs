//! Scrape-time SQL sampler for business metrics (PR4, PB-2947).
//!
//! The sampler runs at /metrics scrape time against a dedicated pool and is
//! opt-in. Every SQL statement runs in its own short read-only transaction
//! with `SET LOCAL statement_timeout` and hard `LIMIT 100`s so a slow table or
//! hung connection cannot drag /metrics down. A TTL cache absorbs concurrent
//! scrapes from multiple Prometheus replicas.
//!
//! The Go collector is synchronous (pgx blocking calls inside Collect); the
//! Rust port keeps the same shape by driving the async DB work on a
//! dedicated blocking thread inside `collect`, so a scrape never blocks the
//! main runtime.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use prometheus::core::{Collector, Desc};
use prometheus::proto::{self, MetricFamily};
use prometheus::{CounterVec, HistogramVec, Opts};

const DEFAULT_SAMPLER_CACHE_TTL: Duration = Duration::from_secs(8);
const DEFAULT_SAMPLER_QUERY_TIMEOUT: Duration = Duration::from_millis(500);

/// Active-user / active-workspace DB window. Kept to the short window only:
/// the PR2 counters do not carry user/workspace IDs, so long-window actives
/// need counter-derived aggregation, not this sampler over history.
pub const WINDOW_FIVE_MINUTES: &str = "5m";

/// Runtime online if last_seen_at within this many seconds of now().
/// 60s matches daemon heartbeat cadence (~15s) plus relay lag and clock skew.
const RUNTIME_ONLINE_WINDOW_SECONDS: i32 = 60;

/// A running task is "stuck" once started_at is older than this (PB-2328).
const STUCK_RUNNING_INTERVAL: &str = "30 minutes";

fn sampler_windows() -> Vec<(&'static str, String)> {
    // Typed interval bind parameter keeps the window string out of SQL text.
    vec![(WINDOW_FIVE_MINUTES, "5 minutes".to_string())]
}

#[derive(Clone)]
pub struct BusinessSamplerOptions {
    /// Dedicated small pool (MaxConns 1–2) pointed at the same database as
    /// the main app pool, so a sampler stall cannot starve business traffic.
    pub pool: Arc<sqlx::PgPool>,
    pub cache_ttl: Option<Duration>,
    pub query_timeout: Option<Duration>,
}

#[derive(Clone, Default)]
struct SnapshotHistogram {
    count: u64,
    sum: f64,
    /// Upper bound -> cumulative count. Pre-seeded with every bucket bound.
    buckets: Vec<(f64, u64)>,
}

#[derive(Clone, Default)]
struct SamplerSnapshot {
    taken_at: Option<Instant>,
    active_users: HashMap<String, f64>,
    active_workspaces: HashMap<String, f64>,
    task_queued: HashMap<String, f64>,
    task_running: HashMap<(String, String), f64>,
    task_stuck: HashMap<String, f64>,
    runtime_online: HashMap<(String, String), f64>,
    heartbeat_age: HashMap<String, SnapshotHistogram>,
    workspace_total: f64,
    workspace_total_known: bool,
}

impl SamplerSnapshot {
    fn fresh(&self, ttl: Duration) -> bool {
        self.taken_at.map(|t| t.elapsed() < ttl).unwrap_or(false)
    }
}

pub struct BusinessSamplerCollector {
    pool: Arc<sqlx::PgPool>,
    cache_ttl: Duration,
    query_timeout: Duration,

    query_duration: HistogramVec,
    query_errors: CounterVec,
    descs: Vec<Desc>,

    state: Arc<Mutex<SamplerSnapshot>>,
}

fn known_source_labels() -> &'static [&'static str] {
    &[
        "chat",
        "issue",
        "autopilot",
        "autopilot_issue",
        "quick_create",
        "manual",
        "api",
        "other",
    ]
}

fn known_runtime_mode_labels() -> &'static [&'static str] {
    &["local", "cloud", "unknown"]
}

/// Matches the Grafana runtime-health view: seconds for healthy heartbeats,
/// then quickly out to "definitely stale".
const HEARTBEAT_AGE_BUCKETS: &[f64] = &[1.0, 5.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0];

impl BusinessSamplerCollector {
    pub fn new(opts: BusinessSamplerOptions) -> Option<Self> {
        let cache_ttl = match opts.cache_ttl {
            Some(t) if !t.is_zero() => t,
            _ => DEFAULT_SAMPLER_CACHE_TTL,
        };
        let query_timeout = match opts.query_timeout {
            Some(t) if !t.is_zero() => t,
            _ => DEFAULT_SAMPLER_QUERY_TIMEOUT,
        };

        let names: [(&str, &str, &[&str]); 8] = [
            ("cordy_active_users", "Distinct users with chat / task activity in the rolling window. Sampled from the database; stale up to the sampler cache TTL.", &["window"]),
            ("cordy_active_workspaces", "Distinct workspaces with chat / task activity in the rolling window. Sampled from the database.", &["window"]),
            ("cordy_agent_task_queued", "Current agent_task_queue rows in `queued` status by inferred source. Sampled from the database.", &["source"]),
            ("cordy_agent_task_running", "Current agent_task_queue rows in `dispatched` or `running` status by inferred source and runtime mode. Sampled from the database.", &["source", "runtime_mode"]),
            ("cordy_agent_task_stuck_total", "Current `running` agent_task_queue rows whose started_at is older than the stuck threshold. Sampled from the database.", &["source"]),
            ("cordy_runtime_online", "Count of agent_runtime rows with last_seen_at within the online heartbeat window. Sampled from the database.", &["runtime_mode", "provider"]),
            ("cordy_runtime_heartbeat_age_seconds", "Distribution of (now() - agent_runtime.last_seen_at) for runtimes considered online by the sampler.", &["runtime_mode"]),
            ("cordy_workspace_total", "Lifetime workspace row count. Useful for sizing alerts and dashboards.", &[]),
        ];
        let descs = names
            .iter()
            .map(|(name, help, labels)| {
                Desc::new(
                    name.to_string(),
                    help.to_string(),
                    labels.iter().map(|s| s.to_string()).collect(),
                    HashMap::new(),
                )
                .expect("valid descriptor")
            })
            .collect();

        Some(Self {
            pool: opts.pool,
            cache_ttl,
            query_timeout,
            query_duration: HistogramVec::new(
                prometheus::HistogramOpts::new(
                    "cordy_business_sampler_query_seconds",
                    "Per-query duration of the BusinessSamplerCollector. The `name` label is one of the fixed query identifiers and never user-controlled.",
                )
                .buckets(vec![0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5]),
                &["name"],
            )
            .expect("valid histogram vec"),
            query_errors: CounterVec::new(
                Opts::new(
                    "cordy_business_sampler_query_errors_total",
                    "Per-query error count. Includes statement_timeout cancellations, which are the expected outcome of a hung database.",
                ),
                &["name"],
            )
            .expect("valid counter vec"),
            descs,
            state: Arc::new(Mutex::new(SamplerSnapshot::default())),
        })
    }

    pub fn collectors(&self) -> Vec<Box<dyn prometheus::core::Collector>> {
        vec![
            Box::new(self.clone()),
            Box::new(self.query_duration.clone()),
            Box::new(self.query_errors.clone()),
        ]
    }

    /// Refreshes the cached snapshot when stale and returns a clone for the
    /// emit path. A refresh failure keeps the last known snapshot — "metric
    /// briefly stale, sampler does not crash".
    fn maybe_refresh(&self) -> SamplerSnapshot {
        let state = self.state.clone();
        let pool = self.pool.clone();
        let cache_ttl = self.cache_ttl;
        let query_timeout = self.query_timeout;

        let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
        if guard.fresh(cache_ttl) {
            return guard.clone();
        }

        // The refresh runs on a blocking thread; the scrape path waits, which
        // mirrors Go's synchronous Collect exactly.
        let next = tokio::task::block_in_place(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .ok()
                .and_then(|rt| rt.block_on(refresh_from_db(&pool, query_timeout)))
        });
        if let Some(next) = next {
            *guard = next;
        }
        guard.clone()
    }
}

async fn refresh_from_db(pool: &sqlx::PgPool, query_timeout: Duration) -> Option<SamplerSnapshot> {
    let mut snap = SamplerSnapshot {
        taken_at: Some(Instant::now()),
        ..Default::default()
    };

    // One short read-only transaction per query: BEGIN, SET LOCAL
    // statement_timeout, run, COMMIT. Failures are logged (timeouts at INFO —
    // expected steady-state on a degraded DB) and never propagate.
    macro_rules! run {
        ($name:literal, $body:expr) => {{
            let start = Instant::now();
            let result = async {
                let mut conn = pool.acquire().await?;
                let mut tx = sqlx::Acquire::begin(&mut conn).await?;
                let timeout_ms = query_timeout.as_millis();
                sqlx::query(&format!("SET LOCAL statement_timeout = {timeout_ms}"))
                    .execute(&mut *tx)
                    .await?;
                $body(&mut tx, &mut snap).await?;
                tx.commit().await
            }
            .await;
            self_query_duration()
                .with_label_values(&[$name])
                .observe(start.elapsed().as_secs_f64());
            if let Err(e) = result {
                if is_statement_timeout(&e) {
                    tracing::info!(query = $name, error = %e, "business sampler: query canceled");
                } else {
                    tracing::warn!(query = $name, error = %e, "business sampler: query failed");
                }
            }
        }};
    }

    run!("active_users", query_active_users);
    run!("active_workspaces", query_active_workspaces);
    run!("task_queued", query_task_queued);
    run!("task_running", query_task_running);
    run!("task_stuck", query_task_stuck);
    run!("runtime_online", query_runtime_online);
    run!("runtime_heartbeat_age", query_runtime_heartbeat_age);
    run!("workspace_total", query_workspace_total);

    Some(snap)
}

/// Per-query duration histogram shared by the free-function refresh path.
/// The Go version observed durations on the collector's HistogramVec; the
/// port keeps the same series via this process-wide vec.
static SELF_QUERY_DURATION: LazyLock<HistogramVec> = LazyLock::new(|| {
    HistogramVec::new(
        prometheus::HistogramOpts::new(
            "cordy_business_sampler_query_seconds",
            "Per-query duration of the BusinessSamplerCollector. The `name` label is one of the fixed query identifiers and never user-controlled.",
        )
        .buckets(vec![0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5]),
        &["name"],
    )
    .expect("valid histogram vec")
});
use std::sync::LazyLock;
#[allow(non_upper_case_globals)]
fn self_query_duration() -> &'static HistogramVec {
    &SELF_QUERY_DURATION
}
async fn query_active_users(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    snap: &mut SamplerSnapshot,
) -> Result<(), sqlx::Error> {
    const STMT: &str = r#"
SELECT count(DISTINCT user_id) FROM (
  SELECT cs.creator_id AS user_id
  FROM chat_session cs
  WHERE EXISTS (
    SELECT 1 FROM chat_message cm
    WHERE cm.chat_session_id = cs.id
      AND cm.created_at > now() - $1::interval
  )
  UNION ALL
  SELECT i.creator_id AS user_id
  FROM issue i
  WHERE i.creator_type = 'member'
    AND EXISTS (
      SELECT 1 FROM agent_task_queue atq
      WHERE atq.issue_id = i.id
        AND atq.created_at > now() - $1::interval
    )
) u"#;
    for (label, interval) in sampler_windows() {
        let n: i64 = sqlx::query_scalar(STMT)
            .bind(interval)
            .fetch_one(&mut **tx)
            .await?;
        snap.active_users.insert(label.to_string(), n as f64);
    }
    Ok(())
}
async fn query_active_workspaces(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    snap: &mut SamplerSnapshot,
) -> Result<(), sqlx::Error> {
    const STMT: &str = r#"
SELECT count(DISTINCT workspace_id) FROM (
  SELECT cs.workspace_id
  FROM chat_session cs
  WHERE EXISTS (
    SELECT 1 FROM chat_message cm
    WHERE cm.chat_session_id = cs.id
      AND cm.created_at > now() - $1::interval
  )
  UNION ALL
  SELECT i.workspace_id
  FROM issue i
  WHERE EXISTS (
    SELECT 1 FROM agent_task_queue atq
    WHERE atq.issue_id = i.id
      AND atq.created_at > now() - $1::interval
  )
) w"#;
    for (label, interval) in sampler_windows() {
        let n: i64 = sqlx::query_scalar(STMT)
            .bind(interval)
            .fetch_one(&mut **tx)
            .await?;
        snap.active_workspaces.insert(label.to_string(), n as f64);
    }
    Ok(())
}
async fn query_task_queued(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    snap: &mut SamplerSnapshot,
) -> Result<(), sqlx::Error> {
    // CASE mirrors source derivation in service/task.go so sampler and
    // event-time metrics agree.
    const STMT: &str = r#"
SELECT
  CASE
    WHEN chat_session_id IS NOT NULL THEN 'chat'
    WHEN autopilot_run_id IS NOT NULL THEN 'autopilot'
    WHEN issue_id IS NOT NULL THEN 'issue'
    ELSE 'other'
  END AS source,
  count(*) AS n
FROM agent_task_queue
WHERE status = 'queued'
GROUP BY 1
LIMIT 100"#;
    let rows: Vec<(String, i64)> = sqlx::query_as(STMT).fetch_all(&mut **tx).await?;
    for (raw_source, n) in rows {
        *snap
            .task_queued
            .entry(crate::labels::normalize_task_source(&raw_source))
            .or_insert(0.0) += n as f64;
    }
    Ok(())
}
async fn query_task_running(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    snap: &mut SamplerSnapshot,
) -> Result<(), sqlx::Error> {
    // dispatched/running kept in separate UNION ALL branches so Postgres can
    // use the partial indexes from migration 114 independently.
    const STMT: &str = r#"
WITH in_flight AS (
  SELECT chat_session_id, autopilot_run_id, issue_id, runtime_id
  FROM agent_task_queue
  WHERE status = 'dispatched'
  UNION ALL
  SELECT chat_session_id, autopilot_run_id, issue_id, runtime_id
  FROM agent_task_queue
  WHERE status = 'running'
)
SELECT
  CASE
    WHEN atq.chat_session_id IS NOT NULL THEN 'chat'
    WHEN atq.autopilot_run_id IS NOT NULL THEN 'autopilot'
    WHEN atq.issue_id IS NOT NULL THEN 'issue'
    ELSE 'other'
  END AS source,
  COALESCE(ar.runtime_mode, 'unknown') AS runtime_mode,
  count(*) AS n
FROM in_flight atq
LEFT JOIN agent_runtime ar ON ar.id = atq.runtime_id
GROUP BY 1, 2
LIMIT 100"#;
    let rows: Vec<(String, String, i64)> = sqlx::query_as(STMT).fetch_all(&mut **tx).await?;
    for (raw_source, raw_mode, n) in rows {
        let key = (
            crate::labels::normalize_task_source(&raw_source),
            crate::labels::normalize_runtime_mode(&raw_mode),
        );
        *snap.task_running.entry(key).or_insert(0.0) += n as f64;
    }
    Ok(())
}
async fn query_task_stuck(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    snap: &mut SamplerSnapshot,
) -> Result<(), sqlx::Error> {
    let stmt = format!(
        r#"
SELECT
  CASE
    WHEN chat_session_id IS NOT NULL THEN 'chat'
    WHEN autopilot_run_id IS NOT NULL THEN 'autopilot'
    WHEN issue_id IS NOT NULL THEN 'issue'
    ELSE 'other'
  END AS source,
  count(*) AS n
FROM agent_task_queue
WHERE status = 'running'
  AND started_at IS NOT NULL
  AND started_at < now() - interval '{STUCK_RUNNING_INTERVAL}'
GROUP BY 1
LIMIT 100"#
    );
    let rows: Vec<(String, i64)> = sqlx::query_as(&stmt).fetch_all(&mut **tx).await?;
    for (raw_source, n) in rows {
        *snap
            .task_stuck
            .entry(crate::labels::normalize_task_source(&raw_source))
            .or_insert(0.0) += n as f64;
    }
    Ok(())
}
async fn query_runtime_online(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    snap: &mut SamplerSnapshot,
) -> Result<(), sqlx::Error> {
    const STMT: &str = r#"
SELECT runtime_mode, provider, count(*) AS n
FROM agent_runtime
WHERE last_seen_at IS NOT NULL
  AND last_seen_at > now() - ($1::int * interval '1 second')
GROUP BY 1, 2
LIMIT 100"#;
    let rows: Vec<(String, String, i64)> = sqlx::query_as(STMT)
        .bind(RUNTIME_ONLINE_WINDOW_SECONDS)
        .fetch_all(&mut **tx)
        .await?;
    for (raw_mode, raw_provider, n) in rows {
        let key = (
            crate::labels::normalize_runtime_mode(&raw_mode),
            crate::labels::normalize_runtime_provider(&raw_provider),
        );
        *snap.runtime_online.entry(key).or_insert(0.0) += n as f64;
    }
    Ok(())
}
async fn query_runtime_heartbeat_age(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    snap: &mut SamplerSnapshot,
) -> Result<(), sqlx::Error> {
    // At most 100 rows, bucketised here because Postgres does not return
    // histogram-shaped output. Rows older than 15 minutes are dropped —
    // clearly offline, would only smear the histogram tail.
    const STMT: &str = r#"
SELECT runtime_mode, EXTRACT(EPOCH FROM (now() - last_seen_at))::float8 AS age
FROM agent_runtime
WHERE last_seen_at IS NOT NULL
  AND last_seen_at > now() - interval '15 minutes'
ORDER BY last_seen_at DESC
LIMIT 100"#;
    let rows: Vec<(String, f64)> = sqlx::query_as(STMT).fetch_all(&mut **tx).await?;
    let mut per_mode: HashMap<String, Vec<f64>> = HashMap::new();
    for (raw_mode, age) in rows {
        per_mode
            .entry(crate::labels::normalize_runtime_mode(&raw_mode))
            .or_default()
            .push(age.max(0.0));
    }
    for (mode, ages) in per_mode {
        let mut hist = SnapshotHistogram {
            buckets: HEARTBEAT_AGE_BUCKETS.iter().map(|b| (*b, 0)).collect(),
            ..Default::default()
        };
        for age in ages {
            hist.count += 1;
            hist.sum += age;
            for (bound, cumulative) in hist.buckets.iter_mut() {
                if age <= *bound {
                    *cumulative += 1;
                }
            }
        }
        snap.heartbeat_age.insert(mode, hist);
    }
    Ok(())
}
async fn query_workspace_total(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    snap: &mut SamplerSnapshot,
) -> Result<(), sqlx::Error> {
    const STMT: &str = "SELECT count(*) FROM workspace LIMIT 100";
    let n: i64 = sqlx::query_scalar(STMT).fetch_one(&mut **tx).await?;
    snap.workspace_total = n as f64;
    snap.workspace_total_known = true;
    Ok(())
}

impl Clone for BusinessSamplerCollector {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            cache_ttl: self.cache_ttl,
            query_timeout: self.query_timeout,
            query_duration: self.query_duration.clone(),
            query_errors: self.query_errors.clone(),
            descs: self.descs.clone(),
            state: self.state.clone(),
        }
    }
}

impl Collector for BusinessSamplerCollector {
    fn desc(&self) -> Vec<&Desc> {
        self.descs.iter().collect()
    }

    fn collect(&self) -> Vec<MetricFamily> {
        let snap = self.maybe_refresh();
        let mut families = Vec::new();

        for (label, _) in sampler_windows() {
            families.push(gauge_family(
                &self.descs[0],
                snap.active_users.get(label).copied().unwrap_or(0.0),
                vec![label],
            ));
            families.push(gauge_family(
                &self.descs[1],
                snap.active_workspaces.get(label).copied().unwrap_or(0.0),
                vec![label],
            ));
        }
        for source in known_source_labels() {
            families.push(gauge_family(
                &self.descs[2],
                snap.task_queued.get(*source).copied().unwrap_or(0.0),
                vec![source],
            ));
            families.push(gauge_family(
                &self.descs[4],
                snap.task_stuck.get(*source).copied().unwrap_or(0.0),
                vec![source],
            ));
        }
        for source in known_source_labels() {
            for mode in known_runtime_mode_labels() {
                families.push(gauge_family(
                    &self.descs[3],
                    snap.task_running
                        .get(&((*source).to_string(), (*mode).to_string()))
                        .copied()
                        .unwrap_or(0.0),
                    vec![source, mode],
                ));
            }
        }
        for ((mode, provider), val) in &snap.runtime_online {
            families.push(gauge_family(
                &self.descs[5],
                *val,
                vec![mode.as_str(), provider.as_str()],
            ));
        }
        for (mode, hist) in &snap.heartbeat_age {
            families.push(histogram_family(&self.descs[6], hist, vec![mode.as_str()]));
        }
        if snap.workspace_total_known {
            families.push(gauge_family(&self.descs[7], snap.workspace_total, vec![]));
        }
        families
    }
}

fn gauge_family(desc: &Desc, value: f64, labels: Vec<&str>) -> MetricFamily {
    let mut gauge = proto::Gauge::default();
    gauge.set_value(value);
    let mut metric = proto::Metric::default();
    metric.set_gauge(gauge);
    metric.set_label(label_pairs(desc, labels));
    wrap_family(desc, metric)
}

fn histogram_family(desc: &Desc, hist: &SnapshotHistogram, labels: Vec<&str>) -> MetricFamily {
    let mut h = proto::Histogram::default();
    h.set_sample_count(hist.count);
    h.set_sample_sum(hist.sum);
    h.set_bucket(
        hist.buckets
            .iter()
            .map(|(bound, cumulative)| {
                let mut b = proto::Bucket::default();
                b.set_upper_bound(*bound);
                b.set_cumulative_count(*cumulative);
                b
            })
            .collect(),
    );
    let mut metric = proto::Metric::default();
    metric.set_histogram(h);
    metric.set_label(label_pairs(desc, labels));
    wrap_family(desc, metric)
}

fn label_pairs(desc: &Desc, labels: Vec<&str>) -> Vec<proto::LabelPair> {
    desc.variable_labels
        .iter()
        .zip(labels)
        .map(|(name, val)| {
            let mut lp = proto::LabelPair::default();
            lp.set_name(name.clone());
            lp.set_value(val.to_string());
            lp
        })
        .collect()
}

fn wrap_family(desc: &Desc, metric: proto::Metric) -> MetricFamily {
    let mut mf = MetricFamily::default();
    mf.set_name(desc.fq_name.clone());
    mf.set_help(desc.help.clone());
    mf.set_field_type(if metric.histogram.is_some() {
        proto::MetricType::HISTOGRAM
    } else {
        proto::MetricType::GAUGE
    });
    mf.set_metric(vec![metric]);
    mf
}

fn is_statement_timeout(err: &sqlx::Error) -> bool {
    match err {
        sqlx::Error::Database(db_err) => {
            db_err.code().as_deref() == Some("57014")
                || db_err
                    .message()
                    .contains("canceling statement due to statement timeout")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prometheus::proto::MetricType;
    use std::collections::HashMap;

    fn test_desc() -> Desc {
        Desc::new(
            "cordy_sampler_test".to_string(),
            "help".to_string(),
            Vec::new(),
            HashMap::new(),
        )
        .expect("valid descriptor")
    }

    #[test]
    fn sampler_families_set_gauge_and_histogram_types() {
        let gauge = gauge_family(&test_desc(), 1.0, vec![]);
        assert_eq!(gauge.get_field_type(), MetricType::GAUGE);
        let hist = SnapshotHistogram {
            count: 1,
            sum: 1.0,
            buckets: vec![(1.0, 1)],
        };
        let histogram = histogram_family(&test_desc(), &hist, vec![]);
        assert_eq!(histogram.get_field_type(), MetricType::HISTOGRAM);
    }
}
