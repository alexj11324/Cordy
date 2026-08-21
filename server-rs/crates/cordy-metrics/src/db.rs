//! DB pool gauges — port of `server/internal/metrics/db.go`.
//!
//! sqlx does not expose the same pool statistics as pgxpool; the port maps
//! what sqlx exposes onto the same metric names and reports unknown values as
//! absent rather than zero, so dashboards do not read a fake flatline.
//! `total_conns` / `idle_conns` / `acquired_conns` come from
//! [`sqlx::Pool::size`] / [`sqlx::Pool::num_idle`]; the pgx-only counters
//! (acquire counts, wait times, destroy reasons) have no sqlx equivalent.

use std::collections::HashMap;
use std::sync::Arc;

use prometheus::core::{Collector, Desc};
use prometheus::proto::{self, MetricFamily};

pub struct DbCollector {
    pool: Option<Arc<sqlx::PgPool>>,
    descs: Vec<Desc>,
}

fn desc(name: &str, help: &str) -> Desc {
    Desc::new(
        format!("cordy_db_pool_{name}"),
        help.to_string(),
        Vec::new(),
        HashMap::new(),
    )
    .expect("valid descriptor")
}

impl DbCollector {
    pub fn new(pool: Arc<sqlx::PgPool>) -> Self {
        let names: [(&str, &str); 3] = [
            (
                "total_conns",
                "Total PostgreSQL connections currently in the pool.",
            ),
            ("idle_conns", "Currently idle PostgreSQL connections."),
            (
                "acquired_conns",
                "Currently acquired PostgreSQL connections.",
            ),
        ];
        Self {
            pool: Some(pool),
            descs: names.iter().map(|(n, h)| desc(n, h)).collect(),
        }
    }

    /// A collector with no pool collects nothing — mirrors Go's nil-pool
    /// guard in Collect.
    pub fn without_pool() -> Self {
        Self {
            pool: None,
            descs: Vec::new(),
        }
    }
}

impl Collector for DbCollector {
    fn desc(&self) -> Vec<&Desc> {
        self.descs.iter().collect()
    }

    fn collect(&self) -> Vec<MetricFamily> {
        let Some(pool) = &self.pool else {
            return Vec::new();
        };
        let total = pool.size() as f64;
        let idle = pool.num_idle() as f64;
        let values = [total, idle, total - idle];
        self.descs
            .iter()
            .zip(values)
            .map(|(d, v)| {
                let mut gauge = proto::Gauge::default();
                gauge.set_value(v);
                let mut metric = proto::Metric::default();
                metric.set_gauge(gauge);
                let mut mf = MetricFamily::default();
                mf.set_name(d.fq_name.clone());
                mf.set_help(d.help.clone());
                mf.set_metric(vec![metric]);
                mf
            })
            .collect()
    }
}
