//! Owned task group for channel event subscribers.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct State {
    closed: bool,
    handles: Vec<tokio::task::JoinHandle<()>>,
}

/// Tracks tasks spawned from synchronous event-bus callbacks so channel
/// shutdown can stop accepting new work and join or abort every in-flight
/// delivery before returning.
#[derive(Clone)]
pub struct RuntimeTasks {
    state: Arc<Mutex<State>>,
}

impl Default for RuntimeTasks {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                closed: false,
                handles: Vec::new(),
            })),
        }
    }
}

impl RuntimeTasks {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts owned work unless shutdown has closed the group. Completed
    /// handles are reaped opportunistically so a long-running server does not
    /// retain one allocation per historical event.
    pub fn spawn<F>(&self, future: F) -> bool
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.closed {
            return false;
        }
        state.handles.retain(|handle| !handle.is_finished());
        state.handles.push(tokio::spawn(future));
        true
    }

    /// Closes admission and waits under one deadline. Timed-out work is
    /// explicitly aborted and joined, so no task outlives this method.
    pub async fn shutdown(&self, timeout: Duration) -> bool {
        let mut handles = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.closed = true;
            std::mem::take(&mut state.handles)
        };
        let joined = async {
            for handle in &mut handles {
                let _ = handle.await;
            }
        };
        if tokio::time::timeout(timeout, joined).await.is_ok() {
            return true;
        }
        for handle in &handles {
            handle.abort();
        }
        for handle in handles {
            let _ = handle.await;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_closes_admission_and_aborts_overdue_work() {
        let tasks = RuntimeTasks::new();
        assert!(tasks.spawn(std::future::pending()));
        assert!(!tasks.shutdown(Duration::from_millis(1)).await);
        assert!(!tasks.spawn(async {}));
    }
}
