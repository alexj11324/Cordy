use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type RuntimeHealthFuture = Pin<Box<dyn Future<Output = bool> + Send>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeHealthState {
    Starting,
    Healthy,
    Degraded,
    Offline,
    Error,
}

impl RuntimeHealthState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Offline => "offline",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeHealthObservation {
    pub state: RuntimeHealthState,
    pub error_code: Option<&'static str>,
    pub error_summary: Option<&'static str>,
}

/// Token-fenced callback supplied by the Supervisor to one channel build.
/// Adapters report only stable codes and pre-written safe summaries; provider
/// errors and credential-bearing URLs never cross this boundary.
#[derive(Clone)]
pub struct RuntimeHealthReporter {
    callback: Arc<dyn Fn(RuntimeHealthObservation) -> RuntimeHealthFuture + Send + Sync>,
}

impl RuntimeHealthReporter {
    pub fn new<F, Fut>(callback: F) -> Self
    where
        F: Fn(RuntimeHealthObservation) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = bool> + Send + 'static,
    {
        Self {
            callback: Arc::new(move |observation| Box::pin(callback(observation))),
        }
    }

    /// Returns whether the observation was durably accepted by the current
    /// token-fenced observer. Adapters may ignore the result when health is
    /// reported repeatedly, but one-shot reporters must use it to retry a
    /// transient persistence failure.
    pub async fn observe(&self, observation: RuntimeHealthObservation) -> bool {
        (self.callback)(observation).await
    }

    pub async fn healthy(&self) -> bool {
        self.observe(RuntimeHealthObservation {
            state: RuntimeHealthState::Healthy,
            error_code: None,
            error_summary: None,
        })
        .await
    }
}

impl std::fmt::Debug for RuntimeHealthReporter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RuntimeHealthReporter(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[tokio::test]
    async fn reporter_forwards_typed_observation() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&seen);
        let reporter = RuntimeHealthReporter::new(move |observation| {
            let output = Arc::clone(&output);
            async move {
                output.lock().unwrap().push(observation);
                true
            }
        });
        assert!(reporter.healthy().await);
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &[RuntimeHealthObservation {
                state: RuntimeHealthState::Healthy,
                error_code: None,
                error_summary: None,
            }]
        );
    }
}
