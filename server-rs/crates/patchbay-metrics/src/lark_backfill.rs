//! Low-cardinality production metrics for the boot-time Lark installation repair.

use prometheus::{CounterVec, Opts};

pub struct LarkBackfillMetrics {
    runs: CounterVec,
    rows: CounterVec,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LarkBackfillReportMetrics {
    pub region_relabelled: u64,
    pub region_errors: u64,
    pub attempted: u64,
    pub filled: u64,
    pub missed: u64,
    pub errored: u64,
    pub raced: u64,
    pub cancelled: bool,
}

impl LarkBackfillMetrics {
    pub fn new() -> Self {
        Self {
            runs: CounterVec::new(
                Opts::new(
                    "patchbay_lark_backfill_runs_total",
                    "Boot-time Lark installation backfill passes by terminal outcome.",
                ),
                &["outcome"],
            )
            .expect("valid counter vec"),
            rows: CounterVec::new(
                Opts::new(
                    "patchbay_lark_backfill_rows_total",
                    "Lark installation backfill row outcomes by bounded operation and outcome.",
                ),
                &["operation", "outcome"],
            )
            .expect("valid counter vec"),
        }
    }

    pub fn record_report(&self, report: LarkBackfillReportMetrics) {
        self.runs
            .with_label_values(&[if report.cancelled {
                "cancelled"
            } else {
                "completed"
            }])
            .inc();
        self.add_rows("region", "relabelled", report.region_relabelled);
        self.add_rows("region", "error", report.region_errors);
        self.add_rows("union_id", "attempted", report.attempted);
        self.add_rows("union_id", "filled", report.filled);
        self.add_rows("union_id", "missed", report.missed);
        self.add_rows("union_id", "error", report.errored);
        self.add_rows("union_id", "raced", report.raced);
    }

    pub fn record_run_error(&self) {
        self.runs.with_label_values(&["error"]).inc();
    }

    fn add_rows(&self, operation: &'static str, outcome: &'static str, count: u64) {
        if count > 0 {
            self.rows
                .with_label_values(&[operation, outcome])
                .inc_by(count as f64);
        }
    }

    pub fn collectors(&self) -> Vec<Box<dyn prometheus::core::Collector>> {
        vec![Box::new(self.runs.clone()), Box::new(self.rows.clone())]
    }
}

impl Default for LarkBackfillMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_bounded_and_reports_count_exactly() {
        let metrics = LarkBackfillMetrics::new();
        metrics.record_report(LarkBackfillReportMetrics {
            region_relabelled: 2,
            attempted: 3,
            filled: 1,
            missed: 1,
            raced: 1,
            ..Default::default()
        });
        let registry = prometheus::Registry::new();
        for collector in metrics.collectors() {
            registry.register(collector).unwrap();
        }
        let families = registry.gather();
        let rows = families
            .iter()
            .find(|family| family.name() == "patchbay_lark_backfill_rows_total")
            .unwrap();
        let mut observed = rows
            .get_metric()
            .iter()
            .map(|metric| {
                let operation = metric
                    .get_label()
                    .iter()
                    .find(|label| label.name() == "operation")
                    .unwrap()
                    .value();
                let outcome = metric
                    .get_label()
                    .iter()
                    .find(|label| label.name() == "outcome")
                    .unwrap()
                    .value();
                (
                    operation.to_owned(),
                    outcome.to_owned(),
                    metric.get_counter().value() as u64,
                )
            })
            .collect::<Vec<_>>();
        observed.sort();
        assert_eq!(
            observed,
            vec![
                ("region".to_owned(), "relabelled".to_owned(), 2),
                ("union_id".to_owned(), "attempted".to_owned(), 3),
                ("union_id".to_owned(), "filled".to_owned(), 1),
                ("union_id".to_owned(), "missed".to_owned(), 1),
                ("union_id".to_owned(), "raced".to_owned(), 1),
            ]
        );
    }
}
