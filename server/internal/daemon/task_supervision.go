package daemon

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/orvilo-ai/orvilo/server/internal/daemon/execenv"
	"github.com/orvilo-ai/orvilo/server/pkg/taskfailure"
)

// tryEnterClaim records the intent to call ClaimTask. Returns true if the
// caller may proceed, false if the auto-update barrier is in effect. Every
// successful call MUST be paired with an exitClaim() on every exit path —
// either right after a failed/empty claim, or via the handleTask goroutine's
// defer once the task is handed off.
func (d *Daemon) tryEnterClaim() bool {
	d.claimMu.Lock()
	defer d.claimMu.Unlock()
	if d.pauseClaims || d.terminalPersistenceFailed.Load() || d.terminalRecoveryFailed.Load() || d.terminalDeliveryBlocked.Load() {
		return false
	}
	d.claimsInFlight++
	return true
}

// exitClaim releases the in-flight claim recorded by tryEnterClaim.
func (d *Daemon) exitClaim() {
	d.claimMu.Lock()
	defer d.claimMu.Unlock()
	d.claimsInFlight--
}

// trySetClaimBarrier atomically pauses new ClaimTask calls if the daemon is
// fully idle (no claims in flight, no tasks running). Returns true if the
// caller now holds the barrier and must release it with releaseClaimBarrier
// on every non-restart exit path; false if the daemon is busy and the caller
// should defer to the next tick. Used by tryAutoUpdate to close the race
// where a task slips in between the cheap pre-fetch idle check and the
// actual upgrade kick-off.
func (d *Daemon) trySetClaimBarrier() bool {
	d.claimMu.Lock()
	defer d.claimMu.Unlock()
	// Refuse when the barrier is already held. Without this the function silently
	// double-acquires: two holders both believe they own it, and whichever
	// finishes first releases it out from under the other. tryBeginServerUpdate
	// makes the same check for the same reason.
	if d.pauseClaims || d.terminalPersistenceFailed.Load() || d.terminalRecoveryFailed.Load() || d.terminalDeliveryBlocked.Load() || d.claimsInFlight > 0 || d.activeTasks.Load() > 0 {
		return false
	}
	d.pauseClaims = true
	return true
}

// releaseClaimBarrier clears the auto-update claim barrier so pollers may
// resume claiming. Called on failure paths only — a successful upgrade leaves
// the barrier set because triggerRestart is about to take the process down
// and clearing it would open a window for new claims during shutdown.
func (d *Daemon) releaseClaimBarrier() {
	d.claimMu.Lock()
	defer d.claimMu.Unlock()
	d.pauseClaims = false
}

// triggerRestart initiates a graceful daemon restart into the binary at
// restartTargetBinary(). The caller (cmd_daemon.go) checks RestartBinary() and
// launches the new process.
//
// Returns false when the target path could not be resolved, so a caller holding
// the claim barrier can release it instead of leaving the daemon paused for a
// restart that will never happen. A restart already scheduled counts as success:
// the process is going down either way.
func (d *Daemon) triggerRestart() bool {
	d.restartMu.Lock()
	defer d.restartMu.Unlock()

	if d.restartBinary != "" {
		d.logger.Debug("daemon restart already scheduled", "new_binary", d.restartBinary)
		return true
	}

	newBin, err := d.restartTargetBinary()
	if err != nil {
		d.logger.Error("could not resolve executable path for restart", "error", err)
		return false
	}

	d.logger.Info("scheduling daemon restart", "new_binary", newBin)
	d.restartBinary = newBin

	// Cancel the main context to trigger graceful shutdown.
	if d.cancelFunc != nil {
		d.cancelFunc()
	}
	return true
}

// restartTargetBinary resolves the path a restart would re-exec.
//
// For brew installs it keeps the stable symlink path (e.g.
// /opt/homebrew/bin/patchbay) so the restarted daemon picks up the new Cellar
// version automatically: on Linux os.Executable() reads /proc/self/exe, which
// the kernel resolves to the Cellar path, and brew cleanup deletes that path
// after an upgrade. For non-brew installs it resolves to the absolute path of
// the replaced binary.
//
// Shared with trySelfReload, which must version-probe the same binary the
// restart would run — probing os.Executable() directly would read the old
// Cellar path under brew and miss the upgrade entirely.
func (d *Daemon) restartTargetBinary() (string, error) {
	newBin, err := resolveSelfExecutable()
	if err != nil {
		return "", err
	}
	// The install method and brew prefix are fixed for the process lifetime;
	// resolve them once so the per-tick reload probe doesn't fork
	// `brew --prefix` every 5 minutes.
	d.brewTargetOnce.Do(func() {
		d.brewInstall = isBrewInstall()
		if !d.brewInstall {
			return
		}
		if brewPrefix := getBrewPrefix(); brewPrefix != "" {
			d.brewTarget = filepath.Join(brewPrefix, "bin", "orvilo")
		} else if prefix := matchKnownBrewPrefix(newBin); prefix != "" {
			d.brewTarget = filepath.Join(prefix, "bin", "orvilo")
		}
	})
	if d.brewInstall {
		if d.brewTarget != "" {
			return d.brewTarget, nil
		}
		d.logger.Warn("brew install detected but prefix could not be resolved; restart may fail",
			"executable", newBin)
		return newBin, nil
	}
	if resolved, err := filepath.EvalSymlinks(newBin); err == nil {
		newBin = resolved
	}
	return newBin, nil
}

// pollLoop runs the machine-level batch claim poller (MUL-4257): a single
// goroutine claims across ALL of the daemon's runtimes per cycle via
// ClaimTasksWSFirst (WS-first, HTTP fallback), replacing the previous
// one-HTTP-poller-per-runtime model. Wake-up signals — a WS task_available /
// catch-up nudge or a runtime-set change — all collapse to one nudge because a
// batch claim already covers every runtime. On shutdown it stops the poller,
// then drains in-flight tasks.
//
// This trades the per-runtime isolation the old model gave (MUL-1744) for a
// single request; the head-of-line risk is bounded by ClaimTasksWSFirst's short
// per-request timeout (WS) / the client's timeout (HTTP fallback), and the
// server-side batch claim is index-backed + short.
func (d *Daemon) pollLoop(ctx context.Context, taskWakeups <-chan taskWakeup) error {
	sem := newTaskSlotSemaphore(d.cfg.MaxConcurrentTasks)
	var taskWG sync.WaitGroup // tracks in-flight handleTask goroutines

	runtimeSetCh, unsub := d.runtimeSet.Subscribe()
	defer unsub()

	wakeup := make(chan struct{}, 1)
	nudge := func() {
		signalPollerWakeup(wakeup)
	}

	pollerCtx, pollerCancel := context.WithCancel(ctx)
	pollerDone := make(chan struct{})
	go func() {
		defer close(pollerDone)
		d.runBatchPoller(pollerCtx, ctx, sem, wakeup, &taskWG)
	}()

	for {
		select {
		case <-ctx.Done():
			d.logger.Info("poll loop stopping, waiting for in-flight tasks", "max_wait", "30s")
			pollerCancel()
			// Wait for the poller to fully return before waiting on taskWG so a
			// poller between ClaimTasksWSFirst and taskWG.Add(1) cannot race
			// taskWG.Wait at a zero counter.
			<-pollerDone
			waitDone := make(chan struct{})
			go func() { taskWG.Wait(); close(waitDone) }()
			select {
			case <-waitDone:
			case <-time.After(30 * time.Second):
				d.logger.Warn("timed out waiting for in-flight tasks")
			}
			return ctx.Err()
		case <-runtimeSetCh:
			// The batch poller re-derives allRuntimeIDs() each cycle; nudge it to
			// pick up a registered/removed runtime promptly.
			nudge()
		case <-taskWakeups:
			// Targeted-runtime and catch-up wakeups both trigger one batch claim
			// across the whole runtime set.
			nudge()
		}
	}
}

// runBatchPoller is the single machine-level claim+dispatch loop. Each cycle it
// acquires whatever execution slots are free (slot-before-claim, so a claimed
// task never sits server-side `dispatched` without local capacity to run it and
// race the dispatch-timeout sweeper), asks the server for up to that many tasks
// across all of the daemon's runtimes in one call, and dispatches each returned
// task — routed to its runtime by handleTask — into a slot.
//
// pollerCtx is cancelled on shutdown. parentCtx is the daemon root ctx passed to
// handleTask so an in-flight task is not killed just because the poll loop is
// stopping mid-flight.
func (d *Daemon) runBatchPoller(pollerCtx, parentCtx context.Context, sem chan int, wakeup chan struct{}, taskWG *sync.WaitGroup) {
	releaseSlots := func(slots []int) {
		for _, sl := range slots {
			sem <- sl
		}
	}

	for {
		if pollerCtx.Err() != nil {
			return
		}

		runtimeIDs := d.allRuntimeIDs()
		if len(runtimeIDs) == 0 {
			if err := sleepWithContextOrWakeup(pollerCtx, d.cfg.PollInterval, wakeup); err != nil {
				return
			}
			continue
		}

		// Acquire at least one slot (blocking briefly), then grab any other free
		// slots so a single batch claim can fill them all.
		slot, acquired, woke, err := waitForTaskSlot(pollerCtx, sem, wakeup, taskSlotWaitTimeout)
		if err != nil {
			return
		}
		if !acquired {
			if woke {
				continue
			}
			if err := sleepWithContextOrWakeup(pollerCtx, capacityBackoff(d.cfg.PollInterval), wakeup); err != nil {
				return
			}
			continue
		}
		slots := append([]int{slot}, drainAvailableSlots(sem, d.cfg.MaxConcurrentTasks-1)...)

		// Auto-update barrier: refuse to claim while an update prepares to roll
		// the process (paired with the re-check in tryAutoUpdate).
		if !d.tryEnterClaim() {
			releaseSlots(slots)
			if err := sleepWithContextOrWakeup(pollerCtx, d.cfg.PollInterval, wakeup); err != nil {
				return
			}
			continue
		}

		tasks, err := d.ClaimTasksWSFirst(pollerCtx, d.cfg.DaemonID, runtimeIDs, len(slots))
		if err != nil {
			d.exitClaim()
			releaseSlots(slots)
			if pollerCtx.Err() == nil {
				d.logger.Warn("batch claim failed", "error", err)
			}
			if err := sleepWithContextOrWakeup(pollerCtx, d.cfg.PollInterval, wakeup); err != nil {
				return
			}
			continue
		}

		// Dispatch each claimed task into a slot. activeTasks is incremented for
		// every dispatched task BEFORE exitClaim so the auto-update barrier never
		// sees a zero-claims / zero-active window between claim and dispatch.
		dispatched := 0
		for i := range tasks {
			if i >= len(slots) || tasks[i] == nil {
				break
			}
			t := *tasks[i]
			slot := slots[i]
			taskTarget := t.IssueID
			if taskTarget == "" && t.ChatSessionID != "" {
				taskTarget = "chat:" + shortID(t.ChatSessionID)
			}
			d.logger.Info("task received", "task", shortID(t.ID), "target", taskTarget)
			taskWG.Add(1)
			d.activeTasks.Add(1)
			if cache, ok := d.repoCache.(interface{ CancelMaintenance() }); ok {
				// A task can reuse an existing worktree and never enter the
				// checkout path that normally preempts repository maintenance.
				// Cancel all low-priority maintenance before the agent starts so
				// direct Git operations cannot overlap it.
				cache.CancelMaintenance()
			}
			go func(t Task, slot int) {
				defer taskWG.Done()
				defer d.activeTasks.Add(-1)
				defer func() {
					// Release local capacity before waking the poller. The task's
					// terminal callback and local cleanup have both finished at this
					// point, so a successor that was previously blocked by agent
					// capacity or per-(issue, agent) serialization can be claimed
					// immediately instead of waiting for PollInterval.
					sem <- slot
					signalPollerWakeup(wakeup)
				}()
				d.handleTask(parentCtx, t, slot)
			}(t, slot)
			dispatched++
		}
		d.exitClaim()
		if dispatched < len(slots) {
			releaseSlots(slots[dispatched:])
		}

		// If we filled every slot, more work may be queued — loop immediately.
		// Otherwise wait for the next wakeup / poll interval.
		if dispatched > 0 && dispatched == len(slots) {
			continue
		}
		if err := sleepWithContextOrWakeup(pollerCtx, d.cfg.PollInterval, wakeup); err != nil {
			return
		}
	}
}

func signalPollerWakeup(wakeup chan<- struct{}) {
	select {
	case wakeup <- struct{}{}:
	default:
	}
}

// drainAvailableSlots non-blockingly pulls up to max additional slots from the
// semaphore, returning immediately when none are free.
func drainAvailableSlots(sem chan int, max int) []int {
	if max <= 0 {
		return nil
	}
	var slots []int
	for len(slots) < max {
		select {
		case s := <-sem:
			slots = append(slots, s)
		default:
			return slots
		}
	}
	return slots
}

func capacityBackoff(pollInterval time.Duration) time.Duration {
	if pollInterval <= 0 || pollInterval > taskSlotCapacityBackoff {
		return taskSlotCapacityBackoff
	}
	return pollInterval
}

func waitForTaskSlot(ctx context.Context, sem chan int, wakeup <-chan struct{}, wait time.Duration) (slot int, acquired, woke bool, err error) {
	select {
	case slot = <-sem:
		return slot, true, false, nil
	case <-ctx.Done():
		return 0, false, false, ctx.Err()
	default:
	}

	if wait <= 0 {
		return 0, false, false, nil
	}

	timer := time.NewTimer(wait)
	defer timer.Stop()
	select {
	case slot = <-sem:
		return slot, true, false, nil
	case <-wakeup:
		return 0, false, true, nil
	case <-ctx.Done():
		return 0, false, false, ctx.Err()
	case <-timer.C:
		return 0, false, false, nil
	}
}

// newTaskSlotSemaphore returns a buffered channel pre-populated with stable
// slot indices [0, n). Receive to acquire a slot, send the same slot back to
// release. Used by pollLoop to expose ORVILO_TASK_SLOT to spawned tasks.
func newTaskSlotSemaphore(maxConcurrentTasks int) chan int {
	sem := make(chan int, maxConcurrentTasks)
	for i := 0; i < maxConcurrentTasks; i++ {
		sem <- i
	}
	return sem
}

// shouldInterruptAgent decides whether the running agent should be cancelled
// based on the latest GetTaskStatus call. Pure function so the decision is
// trivially testable; the polling goroutine in watchTaskCancellation is just
// I/O around it.
//
// Two conditions trigger cancellation:
//
//  1. status is a terminal state — "completed", "failed", or "cancelled"
//     (isAgentTaskTerminal). The server has already finalized the task: user
//     cancel, issue reassignment, the runtime offline sweeper flipping
//     running → failed during a disconnect, or a duplicate execution that
//     already completed it. Letting the local agent run on is pure waste —
//     CompleteAgentTask only accepts status == "running", so its eventual
//     CompleteTask/FailTask callback is guaranteed to fail and just adds log
//     noise. Reusing isAgentTaskTerminal keeps this set in lockstep with the
//     GC's notion of a terminal task.
//  2. err is a 404 with "task not found" — the task row was deleted while
//     the agent was running. Without this we'd let the local agent keep
//     emitting tool calls against a dead task for its full timeout window.
//
// All other errors (transient network, 5xx, ...) intentionally do NOT
// trigger cancellation — the next tick will retry and we don't want a
// flaky link to kill an in-flight agent.
func shouldInterruptAgent(status string, err error) bool {
	if err != nil {
		return isTaskNotFoundError(err)
	}
	return isAgentTaskTerminal(status)
}

// watchTaskCancellation polls the server for the task's status on the given
// interval and returns a channel that is closed when the running agent
// should be interrupted. The polling goroutine stops when ctx is cancelled,
// so callers should pass the runCtx that was set up around the agent run.
func (d *Daemon) watchTaskCancellation(ctx context.Context, taskID string, pollInterval time.Duration, taskLog *slog.Logger) <-chan struct{} {
	cancelled := make(chan struct{})
	// Subscribe to the reconcile broadcaster before launching the inner
	// goroutine. A WS reconnect that fires between the goroutine starting
	// and its first notify() call would otherwise be dropped; the ticker
	// still bounds the worst case, but the whole point of the broadcast is
	// to avoid waiting on that ticker.
	var reconcileCh <-chan struct{}
	if d.reconcile != nil {
		reconcileCh = d.reconcile.notify()
	}
	go func() {
		ticker := time.NewTicker(pollInterval)
		defer ticker.Stop()
		check := func() bool {
			status, err := d.client.GetTaskStatus(ctx, taskID)
			if !shouldInterruptAgent(status, err) {
				return false
			}
			if err != nil {
				taskLog.Info("task gone server-side, interrupting agent", "error", err)
			} else {
				taskLog.Info("task reached terminal state server-side, interrupting agent", "status", status)
			}
			close(cancelled)
			return true
		}
		for {
			select {
			case <-ctx.Done():
				return
			case <-reconcileCh:
				// Refresh the subscription before issuing the request so a
				// second broadcast that overlaps GetTaskStatus is not lost.
				if d.reconcile != nil {
					reconcileCh = d.reconcile.notify()
				}
				if check() {
					return
				}
			case <-ticker.C:
				if check() {
					return
				}
			}
		}
	}()
	return cancelled
}

func (d *Daemon) handleTask(ctx context.Context, task Task, slot int) {
	d.mu.Lock()
	rt, tracked := d.runtimeIndex[task.RuntimeID]
	d.mu.Unlock()
	// The runtime can go offline between the batch claim leaving with its ID and
	// the claimed task arriving here — a below-minimum demotion or a drift
	// convergence to zero both drop rows while a claim is in flight. Reporting it
	// as runtime_offline is what the server already retries on; without this the
	// zero-value Runtime carries an empty provider and the task dies several
	// hundred lines later as `no agent configured for provider ""`, which reads
	// like a misconfigured host and is not retried.
	if !tracked {
		d.logger.Warn("claimed task targets a runtime this daemon no longer hosts; failing it back for retry",
			"task", shortID(task.ID), "runtime_id", task.RuntimeID)
		if err := d.reportTerminalTask(ctx, terminalTaskReport{
			kind:          terminalTaskReportFail,
			taskID:        task.ID,
			claimFence:    task.ClaimFence,
			errorMessage:  "runtime went offline before the task started",
			failureReason: taskfailure.ReasonRuntimeOffline.String(),
		}); err != nil {
			d.logger.Error("fail task callback failed", "task", shortID(task.ID), "error", err)
		}
		return
	}
	provider := rt.Provider
	if providerUsesCredentialBroker(provider) {
		model := ""
		if task.Agent != nil {
			model = task.Agent.Model
		}
		if strings.TrimSpace(task.AuthToken) == "" {
			d.logger.Error("claimed provider task has no task capability token; refusing to start provider",
				"task", shortID(task.ID), "runtime_id", task.RuntimeID, "provider", provider)
			if err := d.reportTerminalTask(ctx, terminalTaskReport{
				kind:          terminalTaskReportFail,
				taskID:        task.ID,
				claimFence:    task.ClaimFence,
				errorMessage:  "provider authorization requires a task capability lease",
				failureReason: "authorization_denied",
			}); err != nil {
				d.logger.Error("provider authorization denial callback failed", "task", shortID(task.ID), "error", err)
			}
			return
		}
		if _, err := d.client.AuthorizeProviderOperation(ctx, task.RuntimeID, task.ID, task.AuthToken, provider, model, 0, true); err != nil {
			failureReason := "authorization_unavailable"
			var requestErr *requestError
			if errors.As(err, &requestErr) && requestErr.StatusCode == http.StatusForbidden {
				failureReason = taskfailure.ReasonAgentProviderAuthOrAccess.String()
			}
			d.logger.Warn("provider pre-operation authorization denied; refusing to spawn provider",
				"task", shortID(task.ID), "runtime_id", task.RuntimeID, "provider", provider,
				"failure_reason", failureReason, "error", err)
			if reportErr := d.reportTerminalTask(ctx, terminalTaskReport{
				kind:          terminalTaskReportFail,
				taskID:        task.ID,
				claimFence:    task.ClaimFence,
				errorMessage:  "provider authorization denied before provider start",
				failureReason: failureReason,
			}); reportErr != nil {
				d.logger.Error("provider authorization failure callback failed", "task", shortID(task.ID), "error", reportErr)
			}
			return
		}
	}

	// Task-scoped logger with short ID for readable concurrent logs.
	taskLog := d.logger.With("task", shortID(task.ID))
	agentName := "agent"
	if task.Agent != nil {
		agentName = task.Agent.Name
	}
	if task.ChatSessionID != "" {
		taskLog.Info("picked chat task", "chat_session", shortID(task.ChatSessionID), "agent", agentName, "provider", provider)
	} else {
		taskLog.Info("picked task", "issue", task.IssueID, "agent", agentName, "provider", provider)
	}
	taskLog.Debug("task context",
		"workspace_id", task.WorkspaceID,
		"runtime_id", task.RuntimeID,
		"agent_id", task.AgentID,
		"repos", len(task.Repos),
		"project_id", task.ProjectID,
		"automation_run_id", task.AutomationRunID,
		"trigger_comment_id", task.TriggerCommentID,
		"resume_session", task.PriorSessionID != "",
		"reuse_workdir", task.PriorWorkDir != "",
	)

	// If the task targets a project_resource of type local_directory that
	// is pinned to this daemon, acquire the path mutex before runner.run
	// so the server-side state machine is dispatched →
	// waiting_local_directory → running rather than backwards-transitioning
	// from running into the wait state. The release is deferred so a panic
	// or early return always frees the lock for the next waiter.
	//
	// StartTask itself now lives in runTask (see issue #3999 race A) and
	// fires only after execenv.Prepare/Reuse has put env.WorkDir on disk,
	// so consumers that read status==running can resolve the workdir path
	// without racing the daemon's os.MkdirAll.
	localRelease, abort := d.acquireLocalDirectoryLockIfNeeded(ctx, task, taskLog)
	if abort {
		return
	}
	if localRelease != nil {
		defer localRelease()
	}

	// Hold a process-wide active-root guard for the rest of this task so
	// the GC loop never sees a window where the env root has neither the
	// in-process guard nor .gc_meta.json (issue #3999 race B). runTask
	// installs its own ref-counted mark/unmark internally; without this
	// outer guard the inner unmark fires when runTask returns, leaving
	// the directory protected only by the 72h orphan TTL through
	// reportTaskResult and execenv.WriteGCMeta below. markActiveEnvRoot
	// is reference-counted, so the duplicate marks runTask installs are
	// correctly nested within these.
	resolvedEnvRoot, resolveRootErr := execenv.ResolveRootDir(taskRootDirParams(d.cfg.WorkspacesRoot, task))
	if resolveRootErr != nil {
		taskLog.Error("resolve stable task env root", "error", resolveRootErr)
	}
	if resolvedEnvRoot != "" {
		d.markActiveEnvRoot(resolvedEnvRoot)
		defer d.unmarkActiveEnvRoot(resolvedEnvRoot)
	}
	if task.PriorWorkDir != "" {
		if priorRoot := filepath.Dir(task.PriorWorkDir); priorRoot != "" && priorRoot != resolvedEnvRoot {
			d.markActiveEnvRoot(priorRoot)
			defer d.unmarkActiveEnvRoot(priorRoot)
		}
	}

	// Create a cancellable context so we can interrupt the running agent
	// when the server signals the task should stop — either the task reached
	// a terminal state (completed/failed/cancelled) or the task row is
	// deleted (404).
	runCtx, runCancel := context.WithCancel(ctx)
	defer runCancel()

	// Poll interval is d.cancelPollInterval (5s in production, reduced in tests
	// via direct field override). Guard against zero so a misconfigured daemon
	// doesn't panic time.NewTicker.
	pollInterval := d.cancelPollInterval
	if pollInterval == 0 {
		pollInterval = 5 * time.Second
	}
	cancelledByPoll := d.watchTaskCancellation(runCtx, task.ID, pollInterval, taskLog)
	go func() {
		select {
		case <-cancelledByPoll:
			runCancel()
		case <-runCtx.Done():
		}
	}()

	result, err := d.runner.run(runCtx, task, provider, slot, taskLog)

	// Report usage before any early return — the agent accumulates tokens
	// whether the task completes, errors, or is cancelled mid-run by the poll
	// goroutine. Both claude.go and codex.go populate result.Usage even when
	// runCtx is cancelled, so dropping this on the cancelled path silently
	// under-reports billing.
	if len(result.Usage) > 0 {
		if usageErr := d.client.ReportTaskUsage(ctx, task.ID, result.Usage); usageErr != nil {
			taskLog.Warn("report task usage failed", "error", usageErr)
		}
	}

	// Check if we were cancelled by the polling goroutine.
	select {
	case <-cancelledByPoll:
		taskLog.Info("task cancelled during execution, discarding result",
			"branch_name", result.BranchName, "error", err)
		// runner.run has returned, so the transcript flush is complete —
		// tell the server it can settle its deferred chat finalization
		// (#5219). The sweeper grace period covers a lost ack's chat settle,
		// but NOT the payload: the branch rides along because the worktree was
		// already finalized before this check, and when Finalize instead
		// ABORTED, the preserved-worktree error is the only pointer left to
		// the agent's work — everything else on this path is discarded. Other
		// run errors stay discarded: on a cancelled run they are expected
		// noise (context canceled, killed process), and persisting them would
		// stamp a bogus reason on every ordinary mid-run cancel.
		ack := TaskCancelAck{
			BranchName:          result.BranchName,
			DurableWorkDir:      result.DurableWorkDir,
			ExecutionProvenance: executionProvenanceFromTaskResult(result),
		}
		var preserved *worktreePreservedError
		if errors.As(err, &preserved) {
			ack.ErrorMessage = preserved.Error()
			ack.FailureReason = "local_directory_error"
		}
		if ackErr := d.client.AckTaskCancelled(ctx, task.ID, ack); ackErr != nil {
			taskLog.Warn("cancel ack failed; server sweeper will finalize", "error", ackErr)
		}
		return
	default:
	}

	if err != nil {
		taskLog.Error("task failed", "error", err)
		// runTask may have reached worktree finalization before returning the
		// error. Preserve any delivery metadata that defer attached to the named
		// result, especially the actual/preserved workdir and delivered branch.
		// MUL-2946: route the bare error string through the canonical
		// classifier so the failure_reason column reflects the actual
		// shape of the failure (provider 5xx, network, process crash,
		// …) rather than the coarse legacy "agent_error" bucket.
		if failErr := d.reportTerminalTask(ctx, terminalTaskReport{
			kind:                terminalTaskReportFail,
			taskID:              task.ID,
			claimFence:          task.ClaimFence,
			errorMessage:        err.Error(),
			branchName:          result.BranchName,
			workDir:             result.WorkDir,
			durableWorkDir:      result.DurableWorkDir,
			failureReason:       taskRunFailureReason(err),
			executionProvenance: executionProvenanceFromTaskResult(result),
		}); failErr != nil {
			taskLog.Error("fail task callback failed", "error", failErr)
		}
		return
	}

	_ = d.client.ReportProgress(ctx, task.ID, "Finishing task", 2, 2)

	// Final pre-completion check: if the server already moved the task to a
	// terminal state (completed/failed/cancelled) or deleted the row
	// outright, skip reporting — the complete/fail callbacks would fail
	// anyway. Reuse shouldInterruptAgent so this guard honors the same
	// signals as the in-flight watcher.
	if status, err := d.client.GetTaskStatus(ctx, task.ID); shouldInterruptAgent(status, err) {
		taskLog.Info("task cancelled during execution, discarding result",
			"status", status, "error", err, "branch_name", result.BranchName)
		// Same contract as the poll-cancelled path above: the transcript is
		// flushed, so let the server settle its deferred chat finalization, and
		// carry the finalized branch so cancelled work stays discoverable. No
		// error to carry here — this branch is only reached with err == nil.
		// This fires for ANY observed terminal status; the server applies the
		// payload only to a cancelled row (status CAS), because for
		// completed/failed rows the complete/fail callback is the
		// authoritative channel and a stale run's late ack must not touch
		// them.
		if ackErr := d.client.AckTaskCancelled(ctx, task.ID, TaskCancelAck{
			BranchName:          result.BranchName,
			DurableWorkDir:      result.DurableWorkDir,
			ExecutionProvenance: executionProvenanceFromTaskResult(result),
		}); ackErr != nil {
			taskLog.Warn("cancel ack failed; server sweeper will finalize", "error", ackErr)
		}
		return
	}

	d.reportTaskResult(ctx, task, result, taskLog)

	// Write GC metadata after the task finishes so the periodic GC loop
	// can look up the parent record (issue / chat session / automation run /
	// task itself for quick-create) later. Written last so that a mid-task
	// crash leaves the directory as an orphan (cleaned up by GCOrphanTTL).
	if result.EnvRoot != "" {
		if meta, ok := gcMetaForTask(task); ok {
			// A local_directory project_resource matched this daemon
			// means the agent ran in the user's own tree. Stamp the
			// meta so the GC loop never tries to RemoveAll envRoot's
			// sibling workdir (which is the user's path) or the envRoot
			// itself (we want output/ and logs/ to linger for forensic
			// access).
			//
			// Worktree mode is excluded: its workdir is a disposable
			// worktree INSIDE envRoot, already removed by Finalize, and
			// the deliverable lives on as a branch in the user's repo.
			// Stamping it would hand every worktree task a permanently
			// exempt env root, so the directory would accumulate one env
			// root per task forever — the exact cost the exemption was
			// meant to trade away for a user's own files.
			if assignment, _ := localDirectoryAssignmentForTask(task, d.cfg.DaemonID); assignment != nil && !assignment.UsesWorktree() {
				meta.LocalDirectory = true
			}
			if err := execenv.WriteGCMeta(result.EnvRoot, meta, taskLog); err != nil {
				taskLog.Warn("write gc meta failed (non-fatal)", "error", err)
			}
		}
	}
}

func providerUsesCredentialBroker(provider string) bool {
	return provider == "codex" || provider == "claude"
}

// worktreePreservedError marks a task error that must survive the cancel path:
// its message names the preserved worktree holding the agent's uncommitted
// work. Every other error on a cancelled run is expected noise (context
// canceled, killed process) and stays discarded; this one is the only pointer
// to real work and rides the cancel ack to the server.
type worktreePreservedError struct{ err error }

func (e *worktreePreservedError) Error() string { return e.err.Error() }
func (e *worktreePreservedError) Unwrap() error { return e.err }

func taskRunFailureReason(err error) string {
	if errors.Is(err, errInvalidTaskIdentity) {
		return taskfailure.ReasonInvalidTaskIdentity.String()
	}
	if errors.Is(err, errTaskPrepareTimeout) {
		return taskfailure.ReasonTimeout.String()
	}
	// Checked after the prepare deadline: when the whole prepare budget ran
	// out, runTask has already collapsed the error into errTaskPrepareTimeout
	// and that classification is the more accurate one. This branch is for the
	// per-skill download deadline firing inside a prepare budget that still had
	// room (MUL-5370).
	if errors.Is(err, errSkillBundleUnavailable) {
		return taskfailure.ReasonSkillBundleUnavailable.String()
	}
	// Structural, not textual: the message ends in "context deadline exceeded",
	// which Classify routes to agent_error.provider_network — "the connection to
	// the model provider dropped, check your network" for a stall that is purely
	// local, plus an auto-retry of a failure that is deterministic on the host.
	// The sentinel survives the preparation helper boundary via
	// preparationErrorKind (#7112).
	if errors.Is(err, execenv.ErrOpenclawCLITimeout) {
		return taskfailure.ReasonRuntimeCLITimeout.String()
	}
	return taskfailure.Classify(err.Error()).String()
}

// acquireLocalDirectoryLockIfNeeded inspects the task's project resources for
// a local_directory pinned to this daemon, validates the path, and takes the
// path mutex. Returns a release callback (nil when no local_directory
// resource applies) and abort=true when the caller must bail without
// starting the task (the helper has already reported the failure to the
// server).
//
// The helper covers four distinct failure modes:
//
//  1. The project_resource JSON is structurally broken — fail the task fast.
//  2. The path fails validation (missing, not a directory, no R/W, system
//     blacklist) — fail the task fast with a user-facing reason.
//  3. The mutex is held by another task — call MarkTaskWaitingLocalDirectory
//     so the row flips to waiting_local_directory while we block on the
//     lock, then return the release callback once we win.
//  4. The blocking wait is cancelled (daemon shutdown, server-side cancel)
//     — fail the task with the ctx error.
func (d *Daemon) acquireLocalDirectoryLockIfNeeded(ctx context.Context, task Task, taskLog *slog.Logger) (release func(), abort bool) {
	if len(task.ProjectResources) == 0 || d.cfg.DaemonID == "" {
		return nil, false
	}
	assignment, err := localDirectoryAssignmentForTask(task, d.cfg.DaemonID)
	if err != nil {
		taskLog.Error("local_directory: resolve resource failed", "error", err)
		if failErr := d.reportTerminalTask(ctx, terminalTaskReport{
			kind:          terminalTaskReportFail,
			taskID:        task.ID,
			claimFence:    task.ClaimFence,
			errorMessage:  err.Error(),
			failureReason: "local_directory_error",
		}); failErr != nil {
			taskLog.Error("fail task after local_directory resolve error", "error", failErr)
		}
		return nil, true
	}
	if assignment == nil {
		return nil, false
	}
	taskLog = taskLog.With("local_directory", assignment.AbsPath)
	// Check the mode before the path: a mode this daemon can't honour is a
	// version-skew problem the user fixes by upgrading, and reporting a path
	// complaint first would send them looking in the wrong place.
	if err := assignment.ValidateExecutionMode(); err != nil {
		taskLog.Error("local_directory: unsupported execution mode", "error", err)
		if failErr := d.reportTerminalTask(ctx, terminalTaskReport{
			kind:          terminalTaskReportFail,
			taskID:        task.ID,
			claimFence:    task.ClaimFence,
			errorMessage:  err.Error(),
			failureReason: "local_directory_error",
		}); failErr != nil {
			taskLog.Error("fail task after local_directory mode check", "error", failErr)
		}
		return nil, true
	}
	if err := validateLocalPath(assignment.AbsPath); err != nil {
		taskLog.Error("local_directory: path validation failed", "error", err)
		if failErr := d.reportTerminalTask(ctx, terminalTaskReport{
			kind:          terminalTaskReportFail,
			taskID:        task.ID,
			claimFence:    task.ClaimFence,
			errorMessage:  err.Error(),
			failureReason: "local_directory_error",
		}); failErr != nil {
			taskLog.Error("fail task after local_directory validation error", "error", failErr)
		}
		return nil, true
	}

	// Worktree mode is the whole point of not serialising: each task gets its
	// own checkout of the repo inside its env root, so there is no shared
	// mutable state on the user's path to protect. Skipping the mutex here is
	// what lets sibling tasks on one directory run concurrently. Path
	// validation above still applies — git needs to write worktree
	// registrations into the user's repo.
	if assignment.UsesWorktree() {
		taskLog.Info("local_directory: worktree mode, skipping path mutex")
		return nil, false
	}

	// A conversation is not a second writer. Everything above still applied —
	// the mode was checked, the path was validated, and the assignment stands,
	// so this task keeps the user's directory as its working directory. Only
	// the queueing is skipped, which is what stops a chat turn from sitting
	// behind a 20-minute build with nothing to contribute to it (issue #7344).
	// See localDirectoryLockExempt for why the mutex does not owe this task a
	// slot.
	if localDirectoryLockExempt(task) {
		taskLog.Info("local_directory: chat task, skipping path mutex")
		return nil, false
	}

	// While the lock is contended the daemon would otherwise sit blocked on
	// the path mutex with no signal back from the server — the main
	// per-task watcher only starts after the lock is acquired. If the user
	// cancels the issue or it gets reassigned during the wait, we need to
	// notice promptly so the daemon slot isn't pinned by a phantom waiter.
	// We spin up the cancellation watcher lazily inside onWait so the
	// no-contention fast path still costs nothing.
	waitCtx, waitCancel := context.WithCancel(ctx)
	defer waitCancel()
	pollInterval := d.cancelPollInterval
	if pollInterval == 0 {
		pollInterval = 5 * time.Second
	}
	var (
		watcherOnce      sync.Once
		prepareLeaseOnce sync.Once
		cancelledByPoll  <-chan struct{}
		stopPrepareLease func()
		waitCounted      bool
	)
	defer func() {
		if waitCounted {
			d.resourceWaitTasks.Add(-1)
		}
	}()
	defer func() {
		if stopPrepareLease != nil {
			stopPrepareLease()
		}
	}()

	onWait := func(holder string) {
		// LocalPathLocker invokes onWait synchronously and at most once for an
		// Acquire call. Count the actual mutex wait even if the best-effort
		// server status update below fails.
		d.resourceWaitTasks.Add(1)
		waitCounted = true
		// Rendered to the user, so it names the directory rather than its path
		// (see localDirectoryAssignment.DisplayName). The absolute path stays in
		// the daemon's own logs, which is where an operator debugging a wedged
		// lock looks for it.
		reason := assignment.DisplayName()
		if holder != "" {
			// Known rough edge: this clause is English and the client renders it
			// inside a localized "Waiting for {reason}" label, so a zh/ja/ko user
			// sees mixed script. Fixing it properly means sending the directory
			// and the holder as separate fields and localizing the join on the
			// client — worth doing if this hint grows, not for one parenthetical.
			reason = fmt.Sprintf("%s (held by task %s)", reason, shortID(holder))
		}
		taskLog.Info("local_directory: waiting on path mutex", "holder", shortID(holder))
		if waitErr := d.client.MarkTaskWaitingLocalDirectory(ctx, task.ID, reason); waitErr != nil {
			// Non-fatal: even if the server-side flag fails to update,
			// we still want to block on the lock and proceed when free.
			// The UI just won't see the explicit "waiting" badge.
			taskLog.Warn("local_directory: mark waiting status failed", "error", waitErr)
		}
		prepareLeaseOnce.Do(func() {
			stopPrepareLease = d.startTaskPrepareLeaseExtender(waitCtx, task, taskLog)
		})
		// Start polling once we actually park. shouldInterruptAgent inside
		// watchTaskCancellation already handles both server-side terminal
		// states (completed/failed/cancelled) and the row-deleted
		// reassignment case (404), which is the full set of "this task
		// shouldn't run anymore" signals we need to react to during the wait.
		watcherOnce.Do(func() {
			cancelledByPoll = d.watchTaskCancellation(waitCtx, task.ID, pollInterval, taskLog)
			go func() {
				select {
				case <-cancelledByPoll:
					waitCancel()
				case <-waitCtx.Done():
				}
			}()
		})
	}
	release, err = d.localPathLocks.Acquire(waitCtx, assignment.RealPath, task.ID, onWait)
	if err != nil {
		// If the wait was cut short because the server finalized the task
		// (terminal state) or deleted the row, the row is already in a
		// terminal state — return silently the same way the run-phase poller
		// does at lines ~2104. Issuing FailTask here would be a no-op at best
		// and a confusing redundant log line at worst.
		if cancelledByPoll != nil {
			select {
			case <-cancelledByPoll:
				taskLog.Info("local_directory: wait aborted by server-side terminal state")
				return nil, true
			default:
			}
		}
		taskLog.Error("local_directory: lock acquire failed", "error", err)
		failureReason := "local_directory_error"
		if errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
			failureReason = "cancelled"
		}
		if failErr := d.reportTerminalTask(ctx, terminalTaskReport{
			kind:          terminalTaskReportFail,
			taskID:        task.ID,
			claimFence:    task.ClaimFence,
			errorMessage:  fmt.Sprintf("local_directory wait cancelled: %s", err.Error()),
			failureReason: failureReason,
		}); failErr != nil {
			taskLog.Error("fail task after local_directory lock cancel", "error", failErr)
		}
		return nil, true
	}
	taskLog.Info("local_directory: lock acquired")
	return release, false
}
