//! Shared daemon activity and exclusion state.
//!
//! One mutex owns claim admission, active-task handoff, and env-root GC
//! reservations. Keeping these transitions together preserves the Go
//! daemon's update barrier and closes task-start/GC check-then-delete races.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

#[derive(Debug, Default)]
struct ActivityState {
    pause_claims: bool,
    claims_in_flight: usize,
    active_tasks: usize,
    active_env_roots: HashMap<PathBuf, usize>,
    deleting_env_roots: HashSet<PathBuf>,
}

/// Authoritative process-wide activity state shared by task execution,
/// auto-update, and garbage collection.
#[derive(Debug, Default)]
pub(crate) struct DaemonActivity {
    state: Mutex<ActivityState>,
    env_root_released: Notify,
}

impl DaemonActivity {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Records intent to issue one batch claim unless an updater has paused
    /// admission. Every successful entry is balanced by guard drop or
    /// [`ClaimGuard::handoff`].
    pub(crate) fn try_enter_claim(self: &Arc<Self>) -> Option<ClaimGuard> {
        let mut state = self.state.lock().unwrap();
        if state.pause_claims {
            return None;
        }
        state.claims_in_flight += 1;
        Some(ClaimGuard {
            activity: Arc::clone(self),
            live: true,
        })
    }

    pub(crate) fn active_tasks(&self) -> usize {
        self.state.lock().unwrap().active_tasks
    }

    pub(crate) fn claims_in_flight(&self) -> usize {
        self.state.lock().unwrap().claims_in_flight
    }

    pub(crate) fn claims_paused(&self) -> bool {
        self.state.lock().unwrap().pause_claims
    }

    /// Atomically acquires the auto-update barrier only when no claim or task
    /// is active. A successful caller owns the barrier until release or
    /// process restart.
    pub(crate) fn try_set_claim_barrier(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.pause_claims || state.claims_in_flight > 0 || state.active_tasks > 0 {
            return false;
        }
        state.pause_claims = true;
        true
    }

    pub(crate) fn release_claim_barrier(&self) {
        self.state.lock().unwrap().pause_claims = false;
    }

    pub(crate) fn is_active_env_root(&self, path: &Path) -> bool {
        self.state
            .lock()
            .unwrap()
            .active_env_roots
            .get(path)
            .is_some_and(|count| *count > 0)
    }

    /// Reserves an inactive env root for a GC mutation. New task handoffs wait
    /// for the returned guard to drop before marking that root active.
    pub(crate) fn reserve_env_root_for_gc(
        self: &Arc<Self>,
        path: &Path,
    ) -> Option<EnvRootGcReservation> {
        if path.as_os_str().is_empty() {
            return None;
        }
        let mut state = self.state.lock().unwrap();
        if state
            .active_env_roots
            .get(path)
            .is_some_and(|count| *count > 0)
            || state.deleting_env_roots.contains(path)
        {
            return None;
        }
        let path = path.to_path_buf();
        state.deleting_env_roots.insert(path.clone());
        Some(EnvRootGcReservation {
            activity: Arc::clone(self),
            path,
        })
    }
}

/// One in-flight claim. Dropping without handoff balances an empty or failed
/// claim; handoff converts it atomically into active task guards.
pub(crate) struct ClaimGuard {
    activity: Arc<DaemonActivity>,
    live: bool,
}

impl ClaimGuard {
    /// Converts this claim into active tasks while keeping update exclusion
    /// continuous. If GC already owns a predicted/prior env root, waits for
    /// that mutation to finish before completing the handoff.
    pub(crate) async fn handoff(
        mut self,
        env_roots_by_task: Vec<Vec<PathBuf>>,
    ) -> Vec<ActiveTaskGuard> {
        loop {
            let wait = self.activity.env_root_released.notified();
            tokio::pin!(wait);
            wait.as_mut().enable();
            {
                let mut state = self.activity.state.lock().unwrap();
                let blocked = env_roots_by_task
                    .iter()
                    .flatten()
                    .any(|path| state.deleting_env_roots.contains(path));
                if !blocked {
                    state.claims_in_flight = state
                        .claims_in_flight
                        .checked_sub(1)
                        .expect("claim handoff without in-flight claim");
                    state.active_tasks += env_roots_by_task.len();
                    for path in env_roots_by_task.iter().flatten() {
                        *state.active_env_roots.entry(path.clone()).or_default() += 1;
                    }
                    self.live = false;
                    return env_roots_by_task
                        .into_iter()
                        .map(|env_roots| ActiveTaskGuard {
                            activity: Arc::clone(&self.activity),
                            env_roots,
                        })
                        .collect();
                }
            }
            wait.as_mut().await;
        }
    }
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        if !self.live {
            return;
        }
        let mut state = self.activity.state.lock().unwrap();
        state.claims_in_flight = state
            .claims_in_flight
            .checked_sub(1)
            .expect("claim guard dropped without in-flight claim");
    }
}

/// Ownership-safe active task count and root protection. Drop runs on normal
/// completion, early return, cancellation, and panic unwind.
pub(crate) struct ActiveTaskGuard {
    activity: Arc<DaemonActivity>,
    env_roots: Vec<PathBuf>,
}

impl Drop for ActiveTaskGuard {
    fn drop(&mut self) {
        let mut state = self.activity.state.lock().unwrap();
        state.active_tasks = state
            .active_tasks
            .checked_sub(1)
            .expect("active task guard dropped without active task");
        for path in &self.env_roots {
            let Some(count) = state.active_env_roots.get_mut(path) else {
                continue;
            };
            if *count <= 1 {
                state.active_env_roots.remove(path);
            } else {
                *count -= 1;
            }
        }
    }
}

pub(crate) struct EnvRootGcReservation {
    activity: Arc<DaemonActivity>,
    path: PathBuf,
}

impl Drop for EnvRootGcReservation {
    fn drop(&mut self) {
        self.activity
            .state
            .lock()
            .unwrap()
            .deleting_env_roots
            .remove(&self.path);
        self.activity.env_root_released.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn claim_handoff_keeps_update_barrier_closed_until_task_drop() {
        let activity = DaemonActivity::new();
        let claim = activity.try_enter_claim().unwrap();
        assert_eq!(activity.claims_in_flight(), 1);
        assert!(!activity.try_set_claim_barrier());

        let mut tasks = claim.handoff(vec![vec![PathBuf::from("/env/a")]]).await;
        assert_eq!(activity.claims_in_flight(), 0);
        assert_eq!(activity.active_tasks(), 1);
        assert!(!activity.try_set_claim_barrier());

        tasks.clear();
        assert_eq!(activity.active_tasks(), 0);
        assert!(activity.try_set_claim_barrier());
        assert!(activity.try_enter_claim().is_none());
        activity.release_claim_barrier();
        assert!(activity.try_enter_claim().is_some());
    }

    #[tokio::test]
    async fn task_handoff_waits_for_gc_reservation() {
        let activity = DaemonActivity::new();
        let root = PathBuf::from("/env/a");
        let reservation = activity.reserve_env_root_for_gc(&root).unwrap();
        let claim = activity.try_enter_claim().unwrap();
        let handoff = tokio::spawn(claim.handoff(vec![vec![root.clone()]]));
        tokio::task::yield_now().await;
        assert!(!handoff.is_finished());

        drop(reservation);
        let tasks = handoff.await.unwrap();
        assert!(activity.is_active_env_root(&root));
        assert!(activity.reserve_env_root_for_gc(&root).is_none());
        drop(tasks);
        assert!(!activity.is_active_env_root(&root));
    }
}
