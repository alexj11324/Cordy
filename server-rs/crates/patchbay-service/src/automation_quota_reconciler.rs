//! Owned lifecycle for the Automation quota reservation reconciler.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::automation::AutomationService;
use crate::automation_failure_monitor::{classify_error, FailureClass, ShutdownOutcome};

const RECONCILE_INTERVAL: Duration = Duration::from_secs(60);
const TERMINAL_RECOVERY_AGE: chrono::Duration = chrono::Duration::minutes(10);
const PARTIAL_RECOVERY_AGE: chrono::Duration = chrono::Duration::hours(6);
const RECONCILE_BATCH: i32 = 100;
const MAX_ATTEMPTS: usize = 3;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(100);
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

pub trait QuotaReconcilerMetrics: Send + Sync {
    fn record(&self, stage: &'static str, outcome: &'static str);
}

impl QuotaReconcilerMetrics for patchbay_metrics::BusinessMetrics {
    fn record(&self, stage: &'static str, outcome: &'static str) {
        self.record_automation_quota_reconciler(stage, outcome);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileOutcome {
    Success {
        settled: usize,
        attempts: usize,
    },
    Failed {
        class: FailureClass,
        attempts: usize,
        message: String,
    },
    Cancelled {
        attempts: usize,
    },
}

#[derive(Clone)]
pub struct AutomationQuotaReconciler {
    service: Arc<AutomationService>,
    metrics: Option<Arc<dyn QuotaReconcilerMetrics>>,
}

impl AutomationQuotaReconciler {
    pub fn new(
        service: Arc<AutomationService>,
        metrics: Option<Arc<dyn QuotaReconcilerMetrics>>,
    ) -> Self {
        Self { service, metrics }
    }

    /// Starts no task when quota admission is disabled. Production owns the
    /// returned handle and explicitly drains it during root shutdown.
    pub fn start(self, cancel: CancellationToken) -> QuotaReconcilerRuntime {
        if !self.service.quota_enabled() {
            return QuotaReconcilerRuntime::disabled(self.metrics);
        }
        let runtime_metrics = self.metrics.clone();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { self.run(task_cancel).await });
        QuotaReconcilerRuntime {
            cancel,
            task: Some(task),
            metrics: runtime_metrics,
        }
    }

    async fn run(self, cancel: CancellationToken) {
        let mut ticker = tokio::time::interval(RECONCILE_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Tokio's first interval tick is immediate, matching the Go loop.
        ticker.tick().await;
        loop {
            match self.run_once(&cancel).await {
                ReconcileOutcome::Success { settled, .. } if settled > 0 => {
                    tracing::info!(settled, "automation quota reconciler settled reservations");
                }
                ReconcileOutcome::Failed { ref message, .. } => {
                    tracing::warn!(error = %message, "automation quota reconciler failed");
                }
                ReconcileOutcome::Cancelled { .. } => return,
                ReconcileOutcome::Success { .. } => {}
            }
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = ticker.tick() => {}
            }
        }
    }

    /// Performs one idempotent CAS-backed reconciliation sweep. A transient
    /// database error may retry the whole sweep because already-settled
    /// reservations cannot decrement or consume quota twice.
    pub async fn run_once(&self, cancel: &CancellationToken) -> ReconcileOutcome {
        let now = Utc::now();
        for attempt in 1..=MAX_ATTEMPTS {
            if cancel.is_cancelled() {
                self.record("reconcile", "cancelled");
                return ReconcileOutcome::Cancelled {
                    attempts: attempt - 1,
                };
            }
            let result = tokio::select! {
                _ = cancel.cancelled() => {
                    self.record("reconcile", "cancelled");
                    return ReconcileOutcome::Cancelled { attempts: attempt - 1 };
                }
                result = self.service.reconcile_quota_reservations(
                    now - TERMINAL_RECOVERY_AGE,
                    now - PARTIAL_RECOVERY_AGE,
                    RECONCILE_BATCH,
                ) => result,
            };
            match result {
                Ok(settled) => {
                    self.record("reconcile", "success");
                    return ReconcileOutcome::Success {
                        settled,
                        attempts: attempt,
                    };
                }
                Err(error) => {
                    let class = classify_error(&error);
                    if class != FailureClass::Retryable || attempt == MAX_ATTEMPTS {
                        self.record(
                            "reconcile",
                            if class == FailureClass::Retryable {
                                "retryable_error"
                            } else {
                                "permanent_error"
                            },
                        );
                        return ReconcileOutcome::Failed {
                            class,
                            attempts: attempt,
                            message: error.to_string(),
                        };
                    }
                    self.record("reconcile", "retryable_error");
                    if !sleep_or_cancel(cancel, RETRY_BASE_DELAY * attempt as u32).await {
                        self.record("reconcile", "cancelled");
                        return ReconcileOutcome::Cancelled { attempts: attempt };
                    }
                }
            }
        }
        unreachable!("bounded reconcile retry loop always returns")
    }

    fn record(&self, stage: &'static str, outcome: &'static str) {
        if let Some(metrics) = &self.metrics {
            metrics.record(stage, outcome);
        }
    }
}

pub struct QuotaReconcilerRuntime {
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
    metrics: Option<Arc<dyn QuotaReconcilerMetrics>>,
}

impl QuotaReconcilerRuntime {
    fn disabled(metrics: Option<Arc<dyn QuotaReconcilerMetrics>>) -> Self {
        Self {
            cancel: CancellationToken::new(),
            task: None,
            metrics,
        }
    }

    pub async fn shutdown(mut self, timeout: Duration) -> ShutdownOutcome {
        self.cancel.cancel();
        let Some(mut task) = self.task.take() else {
            return ShutdownOutcome::Disabled;
        };
        let outcome = match tokio::time::timeout(timeout, &mut task).await {
            Ok(Ok(())) => ShutdownOutcome::Stopped,
            Ok(Err(_)) => ShutdownOutcome::Panicked,
            Err(_) => {
                task.abort();
                let _ = task.await;
                ShutdownOutcome::TimedOut
            }
        };
        if let Some(metrics) = &self.metrics {
            metrics.record(
                "shutdown",
                match outcome {
                    ShutdownOutcome::Stopped => "success",
                    ShutdownOutcome::Panicked => "permanent_error",
                    ShutdownOutcome::TimedOut => "timed_out",
                    ShutdownOutcome::Disabled => "success",
                },
            );
        }
        outcome
    }
}

impl Drop for QuotaReconcilerRuntime {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn sleep_or_cancel(cancel: &CancellationToken, duration: Duration) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(duration) => true,
    }
}
