//! Owned task group for channel event subscribers.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct State {
    closed: bool,
    handles: Vec<tokio::task::JoinHandle<()>>,
}

struct AbortTasksOnDrop {
    handles: Vec<tokio::task::AbortHandle>,
    armed: bool,
}

impl Drop for AbortTasksOnDrop {
    fn drop(&mut self) {
        if self.armed {
            for handle in &self.handles {
                handle.abort();
            }
        }
    }
}

/// Joins owned tasks under one shared deadline. Any task still running at the
/// deadline is aborted and joined. Dropping this future also aborts every task
/// so cancellation cannot silently detach runtime work.
pub async fn shutdown_join_handles(
    mut handles: Vec<tokio::task::JoinHandle<()>>,
    timeout: Duration,
) -> bool {
    let mut abort_on_drop = AbortTasksOnDrop {
        handles: handles.iter().map(|handle| handle.abort_handle()).collect(),
        armed: true,
    };
    let deadline = tokio::time::Instant::now() + timeout;
    while let Some(mut handle) = handles.pop() {
        if tokio::time::timeout_at(deadline, &mut handle)
            .await
            .is_err()
        {
            for handle in &abort_on_drop.handles {
                handle.abort();
            }
            let _ = handle.await;
            for handle in handles {
                let _ = handle.await;
            }
            abort_on_drop.armed = false;
            return false;
        }
    }
    abort_on_drop.armed = false;
    true
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
        let handles = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.closed = true;
            std::mem::take(&mut state.handles)
        };
        shutdown_join_handles(handles, timeout).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_closes_admission_and_aborts_overdue_work() {
        let tasks = RuntimeTasks::new();
        assert!(tasks.spawn(std::future::pending()));
        assert!(tasks.spawn(async {}));
        tokio::task::yield_now().await;
        assert!(!tasks.shutdown(Duration::from_millis(1)).await);
        assert!(!tasks.spawn(async {}));
    }
}
