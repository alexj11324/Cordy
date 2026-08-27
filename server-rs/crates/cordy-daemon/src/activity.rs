//! Shared daemon activity and exclusion state.
//!
//! One mutex owns claim admission, active-task handoff, and env-root GC
//! reservations. Keeping these transitions together preserves the Go
//! daemon's update barrier and closes task-start/GC check-then-delete races.

use std::collections::hash_map::Entry;
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
    active_stores: HashMap<PathBuf, usize>,
    deleting_stores: HashSet<PathBuf>,
}

/// Authoritative process-wide activity state shared by task execution,
/// auto-update, and garbage collection.
#[derive(Debug, Default)]
pub struct DaemonActivity {
    state: Mutex<ActivityState>,
    env_root_released: Notify,
    activity_changed: Notify,
    store_released: Notify,
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

    pub(crate) fn try_claim_barrier(self: &Arc<Self>) -> Option<ClaimBarrierGuard> {
        self.try_set_claim_barrier().then(|| ClaimBarrierGuard {
            activity: Arc::clone(self),
        })
    }

    pub(crate) fn release_claim_barrier(&self) {
        self.state.lock().unwrap().pause_claims = false;
        self.activity_changed.notify_waiters();
    }

    /// Acquires an owned claim barrier and waits until every already-issued
    /// claim has either failed or handed off, and every handed-off task has
    /// exited. Runtime demotion holds this guard across server deregistration
    /// so no task can be claimed against an identity being taken offline.
    pub(crate) async fn pause_claims_until_idle(
        self: &Arc<Self>,
        ctx: &crate::repocache::Ctx,
    ) -> Option<ClaimBarrierGuard> {
        let mut owns_barrier = false;
        loop {
            let changed = self.activity_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = self.state.lock().unwrap();
                if !owns_barrier && !state.pause_claims {
                    state.pause_claims = true;
                    owns_barrier = true;
                }
                if owns_barrier && state.claims_in_flight == 0 && state.active_tasks == 0 {
                    return Some(ClaimBarrierGuard {
                        activity: Arc::clone(self),
                    });
                }
            }
            tokio::select! {
                () = ctx.cancelled() => {
                    if owns_barrier {
                        self.release_claim_barrier();
                    }
                    return None;
                }
                () = changed.as_mut() => {}
            }
        }
    }

    /// Server-triggered update acquisition: pause new claims while allowing an
    /// already-issued claim to finish its active-task handoff.
    pub(crate) async fn pause_claims_when_idle(&self, ctx: &crate::repocache::Ctx) -> bool {
        {
            let mut state = self.state.lock().unwrap();
            if state.pause_claims || state.active_tasks > 0 {
                return false;
            }
            state.pause_claims = true;
        }
        loop {
            let changed = self.activity_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = self.state.lock().unwrap();
                if state.claims_in_flight == 0 {
                    if state.active_tasks == 0 {
                        return true;
                    }
                    state.pause_claims = false;
                    drop(state);
                    self.activity_changed.notify_waiters();
                    return false;
                }
            }
            tokio::select! {
                () = ctx.cancelled() => {
                    self.release_claim_barrier();
                    return false;
                }
                () = changed.as_mut() => {}
            }
        }
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

    pub(crate) fn reserve_store_for_gc(
        self: &Arc<Self>,
        path: &Path,
    ) -> Option<StoreGcReservation> {
        if path.as_os_str().is_empty() {
            return None;
        }
        let mut state = self.state.lock().unwrap();
        if state
            .active_stores
            .get(path)
            .is_some_and(|count| *count > 0)
            || state.deleting_stores.contains(path)
        {
            return None;
        }
        let path = path.to_path_buf();
        state.deleting_stores.insert(path.clone());
        Some(StoreGcReservation {
            activity: Arc::clone(self),
            path,
        })
    }

    /// Provider preparation uses this before mounting persistent session or
    /// memory stores. It waits out a GC deletion reservation and holds the
    /// returned references through finalization.
    pub async fn mark_stores(self: &Arc<Self>, paths: Vec<PathBuf>) -> StoreUseGuard {
        loop {
            let released = self.store_released.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            {
                let mut state = self.state.lock().unwrap();
                if !paths
                    .iter()
                    .any(|path| state.deleting_stores.contains(path))
                {
                    for path in &paths {
                        *state.active_stores.entry(path.clone()).or_default() += 1;
                    }
                    return StoreUseGuard {
                        activity: Arc::clone(self),
                        paths,
                    };
                }
            }
            released.as_mut().await;
        }
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
                    let guards = env_roots_by_task
                        .into_iter()
                        .map(|env_roots| ActiveTaskGuard {
                            activity: Arc::clone(&self.activity),
                            env_roots,
                        })
                        .collect();
                    drop(state);
                    self.activity.activity_changed.notify_waiters();
                    return guards;
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
        drop(state);
        self.activity.activity_changed.notify_waiters();
    }
}

/// Ownership-safe active task count and root protection. Drop runs on normal
/// completion, early return, cancellation, and panic unwind.
pub(crate) struct ActiveTaskGuard {
    activity: Arc<DaemonActivity>,
    env_roots: Vec<PathBuf>,
}

/// Owned pause of task admission. Drop is the only release path so errors and
/// cancellation during a demotion transaction cannot strand the daemon with
/// claims permanently disabled.
pub(crate) struct ClaimBarrierGuard {
    activity: Arc<DaemonActivity>,
}

impl Drop for ClaimBarrierGuard {
    fn drop(&mut self) {
        self.activity.release_claim_barrier();
    }
}

impl Drop for ActiveTaskGuard {
    fn drop(&mut self) {
        let mut state = self.activity.state.lock().unwrap();
        state.active_tasks = state
            .active_tasks
            .checked_sub(1)
            .expect("active task guard dropped without active task");
        for path in &self.env_roots {
            match state.active_env_roots.entry(path.clone()) {
                Entry::Occupied(mut entry) if *entry.get() > 1 => *entry.get_mut() -= 1,
                Entry::Occupied(entry) => {
                    entry.remove();
                }
                Entry::Vacant(_) => {}
            }
        }
        drop(state);
        self.activity.activity_changed.notify_waiters();
    }
}

pub(crate) struct StoreGcReservation {
    activity: Arc<DaemonActivity>,
    path: PathBuf,
}

impl Drop for StoreGcReservation {
    fn drop(&mut self) {
        self.activity
            .state
            .lock()
            .unwrap()
            .deleting_stores
            .remove(&self.path);
        self.activity.store_released.notify_waiters();
    }
}

pub struct StoreUseGuard {
    activity: Arc<DaemonActivity>,
    paths: Vec<PathBuf>,
}

impl Drop for StoreUseGuard {
    fn drop(&mut self) {
        let mut state = self.activity.state.lock().unwrap();
        for path in &self.paths {
            match state.active_stores.entry(path.clone()) {
                Entry::Occupied(mut entry) if *entry.get() > 1 => *entry.get_mut() -= 1,
                Entry::Occupied(entry) => {
                    entry.remove();
                }
                Entry::Vacant(_) => {}
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

    #[test]
    fn nonblocking_claim_barrier_defers_without_pausing_busy_daemon() {
        let activity = DaemonActivity::new();
        let claim = activity.try_enter_claim().unwrap();

        assert!(activity.try_claim_barrier().is_none());
        assert!(!activity.claims_paused());

        drop(claim);
        let barrier = activity.try_claim_barrier().unwrap();
        assert!(activity.claims_paused());
        drop(barrier);
        assert!(!activity.claims_paused());
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

    #[tokio::test]
    async fn server_update_waits_for_claim_and_rejects_its_active_handoff() {
        let activity = DaemonActivity::new();
        let claim = activity.try_enter_claim().unwrap();
        let ctx = crate::repocache::Ctx::new();
        let acquiring = tokio::spawn({
            let activity = Arc::clone(&activity);
            async move { activity.pause_claims_when_idle(&ctx).await }
        });
        tokio::task::yield_now().await;
        assert!(activity.claims_paused());

        let tasks = claim.handoff(vec![Vec::new()]).await;
        assert!(!acquiring.await.unwrap());
        assert!(!activity.claims_paused());
        drop(tasks);
    }

    #[tokio::test]
    async fn owned_claim_barrier_waits_for_claim_handoff_and_active_task_exit() {
        let activity = DaemonActivity::new();
        let claim = activity.try_enter_claim().unwrap();
        let ctx = crate::repocache::Ctx::new();
        let acquiring = tokio::spawn({
            let activity = Arc::clone(&activity);
            async move { activity.pause_claims_until_idle(&ctx).await }
        });
        tokio::task::yield_now().await;
        assert!(activity.claims_paused());
        assert!(activity.try_enter_claim().is_none());

        let tasks = claim.handoff(vec![Vec::new()]).await;
        tokio::task::yield_now().await;
        assert!(!acquiring.is_finished());
        drop(tasks);

        let barrier = acquiring.await.unwrap().unwrap();
        assert!(activity.claims_paused());
        drop(barrier);
        assert!(!activity.claims_paused());
        assert!(activity.try_enter_claim().is_some());
    }

    #[tokio::test]
    async fn store_use_and_gc_reservation_are_mutually_exclusive() {
        let activity = DaemonActivity::new();
        let store = PathBuf::from("/stores/session");
        let use_guard = activity.mark_stores(vec![store.clone()]).await;
        assert!(activity.reserve_store_for_gc(&store).is_none());
        drop(use_guard);

        let reservation = activity.reserve_store_for_gc(&store).unwrap();
        let marking = tokio::spawn({
            let activity = Arc::clone(&activity);
            let store = store.clone();
            async move { activity.mark_stores(vec![store]).await }
        });
        tokio::task::yield_now().await;
        assert!(!marking.is_finished());
        drop(reservation);
        drop(marking.await.unwrap());
    }
}
