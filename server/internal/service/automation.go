package service

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"regexp"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/analytics"
	"github.com/patchbay-ai/patchbay/server/internal/attribution"
	"github.com/patchbay-ai/patchbay/server/internal/dispatch"
	"github.com/patchbay-ai/patchbay/server/internal/entitlement"
	"github.com/patchbay-ai/patchbay/server/internal/events"
	"github.com/patchbay-ai/patchbay/server/internal/issueguard"
	"github.com/patchbay-ai/patchbay/server/internal/issueposition"
	"github.com/patchbay-ai/patchbay/server/internal/issuestatus"
	obsmetrics "github.com/patchbay-ai/patchbay/server/internal/metrics"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

// TxStarter abstracts transaction creation (satisfied by pgxpool.Pool).
type TxStarter interface {
	Begin(ctx context.Context) (pgx.Tx, error)
}

type AutomationService struct {
	Queries      *db.Queries
	TxStarter    TxStarter
	Bus          *events.Bus
	TaskSvc      *TaskService
	Entitlements entitlement.Provider
	QuotaMetrics AutomationQuotaMetrics
}

// DefaultAutomationTriggerTimezone is the timezone used to render Automation
// trigger output when a trigger has no configured timezone or the configured
// timezone fails to load. Exported so the scheduler can use the same default
// when computing next run times.
const DefaultAutomationTriggerTimezone = "UTC"

const automationRecentDuplicateWindow = 60 * time.Second

func NewAutomationService(q *db.Queries, tx TxStarter, bus *events.Bus, taskSvc *TaskService) *AutomationService {
	return &AutomationService{Queries: q, TxStarter: tx, Bus: bus, TaskSvc: taskSvc}
}

// automationRuleConfigSummary captures the substantive (accountability-bearing)
// config of an automation at publish time, stored on each rule-version snapshot for
// audit display (MUL-4302 §7). Cosmetic fields (title / description / issue title
// template) are intentionally excluded — changing them does not transfer
// accountability. Trigger config (cron / webhook / event_filters) lives in a
// separate table and is not inlined here; a trigger edit still republishes the
// rule (recording the editing member + timestamp), the summary just carries the
// automation row's core config.
type automationRuleConfigSummary struct {
	ExecutorType  string `json:"executor_type"`
	ExecutorID    string `json:"executor_id"`
	Status        string `json:"status"`
	ExecutionMode string `json:"execution_mode"`
}

// RecordAutomationRuleVersion appends one rule-version snapshot for a substantive
// publish (MUL-4302 §3.4), recording the publisher and the effective config. Shared
// by the handler publish paths (create / update / trigger edits / archive, run in
// their tx) and the failure monitor's system-pause (a different package). q is the
// caller's *db.Queries (tx-scoped where the caller wants atomicity). publishedByType
// is "member" (with the acting member id) or "system" (with an invalid id, e.g. the
// auto-pause monitor).
func RecordAutomationRuleVersion(ctx context.Context, q *db.Queries, ap db.Automation, publishedByType string, publishedByID pgtype.UUID) error {
	summary, err := json.Marshal(automationRuleConfigSummary{
		ExecutorType:  ap.ExecutorType,
		ExecutorID:    util.UUIDToString(ap.ExecutorID),
		Status:        ap.Status,
		ExecutionMode: ap.ExecutionMode,
	})
	if err != nil {
		return fmt.Errorf("marshal rule version config summary: %w", err)
	}
	if _, err := q.CreateAutomationRuleVersion(ctx, db.CreateAutomationRuleVersionParams{
		AutomationID:     ap.ID,
		WorkspaceID:     ap.WorkspaceID,
		PublishedByType: publishedByType,
		PublishedByID:   publishedByID,
		ConfigSummary:   summary,
	}); err != nil {
		return fmt.Errorf("create automation rule version: %w", err)
	}
	return nil
}

// DispatchAutomation is the core execution entry point.
// It creates a run and either creates an issue or enqueues a direct agent task
// depending on execution_mode.
//
// Before run_only work is queued we run an admission check against the assignee
// agent's runtime: if it is not online, we record a `skipped` run with a
// failure_reason and return without enqueueing. This is the "触发时准入" gate
// from MUL-1899 — without it a paused laptop / offline daemon causes scheduled
// automations to pile thousands of doomed tasks onto agent_task_queue.
//
// create_issue mode is different: its primary contract is a durable audit
// trail. If the assignee has a runtime but that runtime is merely offline,
// dispatch still creates the issue and issue task so the work is visible and
// can be claimed when the runtime returns.
//
// When executor_type='team' the gate runs against the team leader (Path A
// from MUL-2429: Automation-on-team ≈ Automation-on-leader), with the same
// create_issue audit-trail exception for a merely offline leader runtime.
func (s *AutomationService) DispatchAutomation(
	ctx context.Context,
	automation db.Automation,
	triggerID pgtype.UUID,
	source string,
	payload []byte,
) (*db.AutomationRun, error) {
	// No member actor on this entry point (schedule / webhook / api, or a manual
	// trigger without a resolved member): attribution resolves rule_owner. These
	// callers don't surface a per-run reason code to a human, so it is dropped.
	// webhookDeliveryID is invalid here — durable webhook deliveries admit through
	// AdmitAutomationWebhookDelivery instead of this entry point.
	run, _, err := s.dispatchAutomation(ctx, automation, triggerID, source, payload, pgtype.Timestamptz{}, pgtype.UUID{}, pgtype.UUID{}, source+":"+newAutomationIdempotencyKey())
	return run, err
}

// DispatchAutomationManual is the "run now" entry point for a member manually
// triggering an automation. Unlike scheduled / webhook / api dispatch (no human in
// the loop → rule_owner), a manual trigger is a direct human action: the run is
// attributed direct_human to actorUserID, which becomes BOTH its originator
// (authorization) and accountable human (MUL-4302 §4), across both execution modes.
// An invalid actorUserID behaves exactly like DispatchAutomation(source="manual").
func (s *AutomationService) DispatchAutomationManual(
	ctx context.Context,
	automation db.Automation,
	triggerID pgtype.UUID,
	payload []byte,
	actorUserID pgtype.UUID,
) (*db.AutomationRun, dispatch.ReasonCode, error) {
	return s.DispatchAutomationManualWithKey(ctx, automation, triggerID, payload, actorUserID, newAutomationIdempotencyKey())
}

// DispatchAutomationManualWithKey preserves a caller-supplied request key so
// retrying the same HTTP request cannot reserve or execute twice.
func (s *AutomationService) DispatchAutomationManualWithKey(
	ctx context.Context,
	automation db.Automation,
	triggerID pgtype.UUID,
	payload []byte,
	actorUserID pgtype.UUID,
	idempotencyKey string,
) (*db.AutomationRun, dispatch.ReasonCode, error) {
	// The manual path is the one surface that shows a per-run outcome to a human,
	// so it returns the typed reason code decided at the admission source. No
	// webhook delivery on the manual path.
	key := "manual:" + util.UUIDToString(automation.ID) + ":" + idempotencyKey
	return s.dispatchAutomation(ctx, automation, triggerID, "manual", payload, pgtype.Timestamptz{}, pgtype.UUID{}, actorUserID, key)
}

// AdmitAutomationWebhookDelivery creates or reuses the idempotent run for a
// durable webhook delivery without executing its downstream issue/task side
// effect. The HTTP ingress calls this synchronously so the public webhook
// response can retain its 200 accepted/skipped + run_id contract while the
// database-backed worker still owns recoverable dispatch.
func (s *AutomationService) AdmitAutomationWebhookDelivery(
	ctx context.Context,
	automation db.Automation,
	triggerID pgtype.UUID,
	payload []byte,
	deliveryID pgtype.UUID,
) (*db.AutomationRun, error) {
	if !deliveryID.Valid {
		return nil, fmt.Errorf("admit webhook delivery: delivery_id is required")
	}

	existing, err := s.Queries.GetAutomationRunByWebhookDelivery(ctx, deliveryID)
	switch {
	case err == nil:
		return &existing, nil
	case !errors.Is(err, pgx.ErrNoRows):
		return nil, fmt.Errorf("admit webhook delivery: lookup existing run: %w", err)
	}

	// Webhook admission has no member actor → automation principal (rule_owner);
	// the per-run reason code is not surfaced to a human here, so it is dropped.
	if reason, _, skip := s.shouldSkipDispatch(ctx, automation, pgtype.UUID{}); skip {
		run, err := s.recordSkippedRun(
			ctx,
			automation,
			triggerID,
			"webhook",
			payload,
			pgtype.Timestamptz{},
			deliveryID,
			reason,
		)
		if err != nil {
			return s.recoverConcurrentWebhookAdmission(
				ctx,
				deliveryID,
				fmt.Errorf("admit webhook delivery: create skipped run: %w", err),
			)
		}
		return run, nil
	}

	initialStatus := "issue_created"
	if automation.ExecutionMode == "run_only" {
		initialStatus = "running"
	}
	run, _, err := s.createAutomationRunWithQuota(ctx, automation.WorkspaceID, "webhook", "webhook:"+util.UUIDToString(deliveryID), db.CreateAutomationRunParams{
		ID:                dbid.NewV7(),
		AutomationID:       automation.ID,
		TriggerID:         triggerID,
		Source:            "webhook",
		Status:            initialStatus,
		TriggerPayload:    payload,
		TeamID:           automationTeamAttribution(automation),
		WebhookDeliveryID: deliveryID,
	})
	if err != nil {
		return s.recoverConcurrentWebhookAdmission(
			ctx,
			deliveryID,
			fmt.Errorf("admit webhook delivery: create run: %w", err),
		)
	}
	s.captureAutomationRunStarted(automation, run, "webhook")
	return &run, nil
}

func (s *AutomationService) recoverConcurrentWebhookAdmission(
	ctx context.Context,
	deliveryID pgtype.UUID,
	cause error,
) (*db.AutomationRun, error) {
	// Another server replica may have claimed the durable delivery after
	// ingress persisted it but before the admission lookup. The unique
	// delivery/run index chooses one winner; the loser reuses that run.
	var pgErr *pgconn.PgError
	if !errors.As(cause, &pgErr) || pgErr.Code != "23505" {
		return nil, cause
	}
	existing, err := s.Queries.GetAutomationRunByWebhookDelivery(ctx, deliveryID)
	if err == nil {
		return &existing, nil
	}
	if !errors.Is(err, pgx.ErrNoRows) {
		return nil, fmt.Errorf("admit webhook delivery: reload concurrent run: %w", err)
	}
	return nil, cause
}

// DispatchAutomationForWebhookDelivery is the durable webhook worker entry
// point. webhook_delivery_id is persisted on the run and protected by a
// partial unique index, so reclaiming a queued delivery after a process crash
// reuses the original run instead of creating a second issue or task.
func (s *AutomationService) DispatchAutomationForWebhookDelivery(
	ctx context.Context,
	automation db.Automation,
	triggerID pgtype.UUID,
	payload []byte,
	deliveryID pgtype.UUID,
) (*db.AutomationRun, error) {
	run, err := s.AdmitAutomationWebhookDelivery(ctx, automation, triggerID, payload, deliveryID)
	if err != nil {
		return nil, err
	}
	if isAutomationRunComplete(*run) {
		if automation.ExecutionMode == "create_issue" && run.IssueID.Valid {
			if repairErr := s.ensureWebhookCreateIssueTask(ctx, automation, *run); repairErr != nil {
				return run, repairErr
			}
		}
		return run, nil
	}

	// A run_only task may have committed immediately before the process died
	// while linking task_id back to the run. Repair that linkage and wake the
	// daemon; otherwise continue the same partial run below.
	if automation.ExecutionMode == "run_only" && !run.TaskID.Valid {
		repaired, found, repairErr := s.repairAutomationRunTaskLink(ctx, *run)
		if repairErr != nil {
			return run, fmt.Errorf("dispatch for webhook delivery: %w", repairErr)
		}
		if found {
			return repaired, nil
		}
	}
	// Webhook worker dispatch has no member actor and no human reason-code
	// surface, so actorUserID is invalid and the reason code is dropped.
	dispatched, _, err := s.dispatchAutomationRun(ctx, automation, triggerID, "webhook", run, pgtype.UUID{})
	return dispatched, err
}

// ensureWebhookCreateIssueTask repairs the create_issue crash window after the
// issue/run transaction commits but before the ordinary task enqueue commits.
// Any existing issue task is sufficient evidence that ownership has already
// moved downstream; otherwise enqueue exactly the same assignee path used by
// the original dispatch.
func (s *AutomationService) ensureWebhookCreateIssueTask(ctx context.Context, automation db.Automation, run db.AutomationRun) error {
	tasks, err := s.Queries.ListTasksByIssue(ctx, run.IssueID)
	if err != nil {
		return fmt.Errorf("dispatch for webhook delivery: inspect issue tasks: %w", err)
	}
	if len(tasks) > 0 {
		return nil
	}
	issue, err := s.Queries.GetIssue(ctx, run.IssueID)
	if err != nil {
		return fmt.Errorf("dispatch for webhook delivery: load linked issue: %w", err)
	}
	if effective := issuestatus.Effective(ctx, s.Queries, issue.WorkspaceID, issue.Status); effective != "todo" && effective != "in_progress" {
		return nil
	}
	if automation.ExecutorType == "team" {
		leader, _, err := s.resolveAutomationLeader(ctx, automation)
		if err != nil {
			return fmt.Errorf("dispatch for webhook delivery: resolve team leader: %w", err)
		}
		if _, err := s.TaskSvc.EnqueueTaskForTeamLeader(ctx, issue, leader.ID, automation.ExecutorID, pgtype.UUID{}); err != nil {
			return fmt.Errorf("dispatch for webhook delivery: repair team task: %w", err)
		}
		return nil
	}
	if _, err := s.TaskSvc.EnqueueTaskForIssue(ctx, issue); err != nil {
		return fmt.Errorf("dispatch for webhook delivery: repair issue task: %w", err)
	}
	return nil
}

// repairAutomationRunTaskLink closes the run_only crash window where task
// creation committed but automation_run.task_id did not. Finding any task is
// proof that downstream ownership already moved; active work is re-woken and
// terminal work is replayed through the normal finalizer instead of duplicated.
func (s *AutomationService) repairAutomationRunTaskLink(ctx context.Context, run db.AutomationRun) (*db.AutomationRun, bool, error) {
	task, err := s.Queries.GetAutomationTaskByRun(ctx, run.ID)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, false, nil
	}
	if err != nil {
		return nil, false, fmt.Errorf("lookup linked task: %w", err)
	}
	updated, err := s.Queries.UpdateAutomationRunRunning(ctx, db.UpdateAutomationRunRunningParams{
		ID:     run.ID,
		TaskID: task.ID,
	})
	if err != nil {
		return nil, false, fmt.Errorf("repair task linkage: %w", err)
	}
	switch task.Status {
	case "completed", "failed", "cancelled":
		s.SyncRunFromTask(ctx, task)
		updated, err = s.Queries.GetAutomationRun(ctx, run.ID)
		if err != nil {
			return nil, false, fmt.Errorf("reload terminal repaired run: %w", err)
		}
	default:
		s.TaskSvc.NotifyTaskEnqueued(ctx, task)
	}
	return &updated, true, nil
}

// DispatchAutomationForPlan is the entry point for scheduled triggers that
// already know the canonical UTC plan_time of the occurrence they are
// firing. The plan_time is persisted on automation_run.planned_at, and the
// (trigger_id, planned_at) partial unique index — combined with this
// method's idempotent lookup — guarantees that the SAME planned occurrence
// cannot produce two SUCCESSFUL runs even if a stale-steal in
// sys_cron_executions re-enters this method after a prior attempt.
//
// Semantics for an already-existing run at (trigger_id, planned_at):
//
//   - If the existing run is COMPLETE (terminal status, or in-flight
//     with the appropriate downstream linkage — issue_id for
//     create_issue, task_id for run_only), it is returned unchanged.
//     The handler then writes SUCCESS in sys_cron_executions; no
//     duplicate issue/task is produced.
//   - If the existing run is in a PARTIAL state (a prior attempt
//     wrote the run row but crashed before creating its downstream
//     issue/task), it is marked FAILED with a recovery reason and
//     its planned_at is cleared, releasing the partial-unique slot.
//     Dispatch then proceeds normally and creates a fresh run at the
//     same plan_time. Without this branch, a crash-during-dispatch
//     would let a subsequent retry see the in-flight run, return it
//     unchanged, and let the scheduler mark the occurrence SUCCESS
//     without an actual issue/task ever being created (#4443 review).
//
// triggerID and plannedAt MUST both be valid; passing zero values
// would silently disable the idempotency guard. Manual / webhook /
// api callers should use DispatchAutomation instead.
func (s *AutomationService) DispatchAutomationForPlan(
	ctx context.Context,
	automation db.Automation,
	triggerID pgtype.UUID,
	source string,
	payload []byte,
	plannedAt time.Time,
) (*db.AutomationRun, error) {
	if !triggerID.Valid {
		return nil, fmt.Errorf("dispatch for plan: trigger_id is required")
	}
	if plannedAt.IsZero() {
		return nil, fmt.Errorf("dispatch for plan: planned_at is required")
	}
	plannedTS := pgtype.Timestamptz{Time: plannedAt.UTC(), Valid: true}

	// Fast path: prior attempt already created a run for this exact
	// occurrence. The partial unique index uq_automation_run_trigger_planned
	// would also reject a duplicate INSERT, but doing the lookup up
	// front lets us short-circuit on a complete run and gives us a
	// chance to recover a partial run before retrying.
	existing, err := s.Queries.GetAutomationRunByTriggerAndPlanned(ctx, db.GetAutomationRunByTriggerAndPlannedParams{
		TriggerID: triggerID,
		PlannedAt: plannedTS,
	})
	switch {
	case err == nil && isAutomationRunComplete(existing):
		// A prior attempt produced a complete run. Hand it back so the
		// handler can record SUCCESS in sys_cron_executions without
		// duplicating any downstream side effect.
		return &existing, nil

	case err == nil:
		if automation.ExecutionMode == "run_only" && !existing.TaskID.Valid {
			repaired, found, repairErr := s.repairAutomationRunTaskLink(ctx, existing)
			if repairErr != nil {
				return nil, fmt.Errorf("dispatch for plan: %w", repairErr)
			}
			if found {
				return repaired, nil
			}
		}
		// Partial-state run from a crashed attempt. Mark it failed
		// (with a recovery reason) and release its partial-unique
		// slot so the fresh dispatch below can create a new row.
		slog.Warn("automation dispatch for plan: recovering partial run",
			"run_id", util.UUIDToString(existing.ID),
			"trigger_id", util.UUIDToString(triggerID),
			"planned_at", plannedAt.UTC().Format(time.RFC3339),
			"status", existing.Status,
			"issue_set", existing.IssueID.Valid,
			"task_set", existing.TaskID.Valid,
		)
		recovered, err := s.recoverPartialAutomationRun(ctx, existing)
		if err != nil {
			return nil, fmt.Errorf("dispatch for plan: recover partial run: %w", err)
		}
		if !recovered {
			return nil, fmt.Errorf("dispatch for plan: partial run changed concurrently; retry")
		}
		// Fall through to a fresh dispatch below.

	case !errors.Is(err, pgx.ErrNoRows):
		return nil, fmt.Errorf("dispatch for plan: lookup existing run: %w", err)
	}

	// Scheduled dispatch has no member actor → rule_owner attribution, and no
	// human surface for a per-run reason code, so it is dropped. No webhook
	// delivery on the scheduled-plan path.
	key := "schedule:" + util.UUIDToString(triggerID) + ":" + plannedAt.UTC().Format(time.RFC3339Nano)
	run, _, err := s.dispatchAutomation(ctx, automation, triggerID, source, payload, plannedTS, pgtype.UUID{}, pgtype.UUID{}, key)
	return run, err
}

// isAutomationRunComplete decides whether an existing automation_run row
// for (trigger_id, planned_at) is safe to reuse on a stale-steal retry.
//
// A run is "complete" if either:
//
//   - It is in a terminal state (completed / failed / skipped). Nothing
//     more to do downstream; the caller can return it as-is.
//
//   - It is in-flight in a state whose downstream side effect is
//     observable:
//
//   - issue_created with a valid issue_id — the issue exists and
//     the issue-event listener owns task creation from here.
//
//   - running with a valid task_id — the task is queued, the
//     listener will close the run when the task terminates.
//
// Anything else — most importantly issue_created/running with NULL
// issue_id/task_id, or the brief 'pending' state — is a partial run:
// the run row was inserted before the dispatch path could create the
// downstream resource, and a stale-steal retry MUST NOT treat it as
// complete (#4443 review).
func isAutomationRunComplete(run db.AutomationRun) bool {
	switch run.Status {
	case "completed", "failed", "skipped":
		return true
	case "issue_created":
		return run.IssueID.Valid
	case "running":
		return run.TaskID.Valid
	default:
		return false
	}
}

// dispatchAutomation is the shared core of the two public Dispatch entry
// points. plannedAt is the canonical UTC plan_time for scheduled triggers;
// for manual / webhook / api dispatch it is the zero pgtype.Timestamptz and
// the resulting automation_run row has planned_at IS NULL. webhookDeliveryID
// is set only by the durable webhook worker.
func (s *AutomationService) dispatchAutomation(
	ctx context.Context,
	automation db.Automation,
	triggerID pgtype.UUID,
	source string,
	payload []byte,
	plannedAt pgtype.Timestamptz,
	webhookDeliveryID pgtype.UUID,
	actorUserID pgtype.UUID,
	idempotencyKey string,
) (*db.AutomationRun, dispatch.ReasonCode, error) {
	if reason, code, skip := s.shouldSkipDispatch(ctx, automation, actorUserID); skip {
		run, err := s.recordSkippedRun(ctx, automation, triggerID, source, payload, plannedAt, webhookDeliveryID, reason)
		return run, code, err
	}

	// Determine initial status based on execution mode.
	initialStatus := "issue_created"
	if automation.ExecutionMode == "run_only" {
		initialStatus = "running"
	}

	run, reused, err := s.createAutomationRunWithQuota(ctx, automation.WorkspaceID, source, idempotencyKey, db.CreateAutomationRunParams{
		ID:                dbid.NewV7(),
		AutomationID:       automation.ID,
		TriggerID:         triggerID,
		Source:            source,
		Status:            initialStatus,
		TriggerPayload:    payload,
		TeamID:           automationTeamAttribution(automation),
		PlannedAt:         plannedAt,
		WebhookDeliveryID: webhookDeliveryID,
	})
	if err != nil {
		var quotaErr *AutomationQuotaExceededError
		if errors.As(err, &quotaErr) && source == "schedule" {
			skipped, skipErr := s.recordSkippedRun(ctx, automation, triggerID, source, payload, plannedAt, webhookDeliveryID, quotaErr.Error(), dispatch.ReasonQuotaExceeded)
			return skipped, dispatch.ReasonQuotaExceeded, skipErr
		}
		return nil, dispatch.ReasonInternalError, fmt.Errorf("create run: %w", err)
	}
	if reused {
		return &run, dispatch.ReasonCode(run.ReasonCode.String), nil
	}
	s.captureAutomationRunStarted(automation, run, source)
	return s.dispatchAutomationRun(ctx, automation, triggerID, source, &run, actorUserID)
}

// dispatchAutomationRun performs the downstream side effect for an already
// persisted run. Keeping creation separate lets the webhook worker resume the
// same idempotency-anchored run after a crash between run creation and issue
// or task creation.
func (s *AutomationService) dispatchAutomationRun(
	ctx context.Context,
	automation db.Automation,
	triggerID pgtype.UUID,
	source string,
	run *db.AutomationRun,
	actorUserID pgtype.UUID,
) (*db.AutomationRun, dispatch.ReasonCode, error) {
	switch automation.ExecutionMode {
	case "create_issue":
		triggerTimezone := s.resolveAutomationTriggerTimezone(ctx, triggerID)
		if err := s.dispatchCreateIssue(ctx, automation, run, triggerTimezone, actorUserID); err != nil {
			if skipped, code := s.handleDispatchSkip(ctx, automation, run, err); skipped != nil {
				return skipped, code, nil
			}
			s.failRun(ctx, run.ID, err.Error())
			s.captureAutomationRunFailed(automation, *run, source, err.Error())
			return run, dispatchFailReasonCode(err), fmt.Errorf("dispatch create_issue: %w", err)
		}
	case "run_only":
		if err := s.dispatchRunOnly(ctx, automation, run, actorUserID); err != nil {
			if skipped, code := s.handleDispatchSkip(ctx, automation, run, err); skipped != nil {
				return skipped, code, nil
			}
			s.failRun(ctx, run.ID, err.Error())
			s.captureAutomationRunFailed(automation, *run, source, err.Error())
			return run, dispatchFailReasonCode(err), fmt.Errorf("dispatch run_only: %w", err)
		}
	default:
		s.failRun(ctx, run.ID, "unknown execution_mode: "+automation.ExecutionMode)
		s.captureAutomationRunFailed(automation, *run, source, "unknown execution_mode: "+automation.ExecutionMode)
		return run, dispatch.ReasonInternalError, fmt.Errorf("unknown execution_mode: %s", automation.ExecutionMode)
	}

	// Update last_run_at on the automation.
	s.Queries.UpdateAutomationLastRunAt(ctx, automation.ID)

	// Publish run start event.
	s.Bus.Publish(events.Event{
		Type:        protocol.EventAutomationRunStart,
		WorkspaceID: util.UUIDToString(automation.WorkspaceID),
		ActorType:   "system",
		Payload: map[string]any{
			"run_id":       util.UUIDToString(run.ID),
			"automation_id": util.UUIDToString(automation.ID),
			"source":       source,
			"status":       run.Status,
		},
	})

	return run, "", nil
}

// dispatchFailReasonCode types a dispatch error that fell through to failRun.
// It inspects the error with typed checks (never substring matching): a
// fail-closed attribution refusal is attribution_blocked; everything else is an
// unclassified internal error.
func dispatchFailReasonCode(err error) dispatch.ReasonCode {
	if errors.Is(err, ErrAttributionFailClosed) {
		return dispatch.ReasonAttributionBlocked
	}
	return dispatch.ReasonInternalError
}

// dispatchCreateIssue creates an issue and enqueues a task for the agent.
//
// When the automation is assigned to a team (Path A from MUL-2429), the
// created issue inherits executor_type='team' + executor_id=team. The
// existing issue listener chain (shouldEnqueueTeamLeaderOnExecutor →
// enqueueTeamLeaderTask) then routes the work to the team leader, exactly
// as a human manually assigning the issue to that team would.
//
// Creator on the issue is always the agent that will actually do the work
// (the resolved leader for a team automation, otherwise the assignee agent
// itself), so activity / mentions render with the right author identity.
func (s *AutomationService) dispatchCreateIssue(ctx context.Context, ap db.Automation, run *db.AutomationRun, triggerTimezone string, actorUserID pgtype.UUID) error {
	leader, _, err := s.resolveAutomationLeader(ctx, ap)
	if err != nil {
		return fmt.Errorf("resolve leader: %w", err)
	}
	issueCountPolicy := ResolveIssueCountPolicy(ctx, s.Entitlements, ap.WorkspaceID)

	tx, err := s.TxStarter.Begin(ctx)
	if err != nil {
		return fmt.Errorf("begin tx: %w", err)
	}
	defer tx.Rollback(ctx)

	qtx := s.Queries.WithTx(tx)

	title := s.interpolateTemplate(ap, *run, triggerTimezone)
	description := s.buildIssueDescription(ap, *run, triggerTimezone)

	// Refresh the automation row at dispatch time so we use the current project
	// binding instead of any stale snapshot the caller may have cached.
	currentAutomation, err := qtx.GetAutomationInWorkspace(ctx, db.GetAutomationInWorkspaceParams{
		ID:          ap.ID,
		WorkspaceID: ap.WorkspaceID,
	})
	if err != nil {
		return fmt.Errorf("refresh automation: %w", err)
	}
	projectID := currentAutomation.ProjectID

	if duplicate, found, err := issueguard.LockAndFindRecentAutomationDuplicate(
		ctx, qtx, ap.WorkspaceID, ap.ID, projectID, title, automationRecentDuplicateWindow,
	); err != nil {
		return fmt.Errorf("recent duplicate guard: %w", err)
	} else if found {
		return &errDispatchSkipped{reason: "recent duplicate automation issue: " + util.UUIDToString(duplicate.ID), code: dispatch.ReasonAlreadyActive}
	}

	issueNumber, err := AllocateIssueNumber(ctx, qtx, ap.WorkspaceID, issueCountPolicy)
	if err != nil {
		var limitErr *IssueLimitReachedError
		if errors.As(err, &limitErr) {
			return &errDispatchSkipped{
				reason: "workspace has reached its issue limit",
				code:   dispatch.ReasonIssueLimitReached,
			}
		}
		return fmt.Errorf("allocate issue number: %w", err)
	}

	newPosition, err := issueposition.NextTopPosition(ctx, tx, ap.WorkspaceID, "todo")
	if err != nil {
		return fmt.Errorf("get next issue position: %w", err)
	}

	issue, err := qtx.CreateIssueWithOrigin(ctx, db.CreateIssueWithOriginParams{
		ID:           dbid.NewV7(),
		WorkspaceID:  ap.WorkspaceID,
		Title:        title,
		Description:  description,
		Status:       "todo",
		Priority:     "none",
		ExecutorType: pgtype.Text{String: ap.ExecutorType, Valid: true},
		ExecutorID:   ap.ExecutorID,
		// The agent that the automation dispatches to is the issue's creator,
		// not the human who originally configured the automation. The latter
		// is captured separately via origin_type=automation + origin_id. For
		// team-assigned automations, the creator is the resolved leader —
		// the same agent the issue listener will end up enqueueing.
		CreatorType:   "agent",
		CreatorID:     leader.ID,
		ParentIssueID: pgtype.UUID{},
		Position:      newPosition,
		StartDate:     pgtype.Date{},
		DueDate:       pgtype.Date{},
		Number:        issueNumber,
		ProjectID:     projectID,
		OriginType:    pgtype.Text{String: "automation", Valid: true},
		OriginID:      ap.ID,
	})
	if err != nil {
		return fmt.Errorf("create issue: %w", err)
	}

	// Fan out the default subscriber template inside the same tx as the
	// issue insert, before EventIssueCreated fires — so notification
	// listeners see the full subscriber set on the first event instead of
	// racing the listener that would otherwise hydrate the template.
	templateSubs, err := qtx.ListAutomationSubscribers(ctx, ap.ID)
	if err != nil {
		return fmt.Errorf("list automation subscribers: %w", err)
	}
	for _, sub := range templateSubs {
		if _, err := qtx.AddIssueSubscriber(ctx, db.AddIssueSubscriberParams{
			IssueID:  issue.ID,
			UserType: sub.UserType,
			UserID:   sub.UserID,
			Reason:   "automation",
		}); err != nil {
			return fmt.Errorf("add automation subscriber to issue: %w", err)
		}
	}

	// Link the run inside the same tx as the issue insert. This makes the
	// recent-duplicate guard count only fully observable automation issues and
	// avoids a crash window where recovery would see an orphan issue but no
	// linked run.
	updatedRun, err := qtx.UpdateAutomationRunIssueCreated(ctx, db.UpdateAutomationRunIssueCreatedParams{
		ID:      run.ID,
		IssueID: issue.ID,
	})
	if err != nil {
		return fmt.Errorf("link run to issue: %w", err)
	}
	*run = updatedRun
	if _, err := settleAutomationQuota(ctx, qtx, run.QuotaReservationID, true); err != nil {
		return fmt.Errorf("consume quota reservation: %w", err)
	}

	if err := tx.Commit(ctx); err != nil {
		return fmt.Errorf("commit tx: %w", err)
	}

	// Publish issue:created so the existing event chain fires
	// (subscriber listeners, activity listeners, notification listeners). For
	// team automations, this is what triggers shouldEnqueueTeamLeaderOnExecutor
	// → enqueueTeamLeaderTask — no separate team-routing code needed here.
	prefix := s.getIssuePrefix(ap.WorkspaceID)
	s.Bus.Publish(events.Event{
		Type:        protocol.EventIssueCreated,
		WorkspaceID: util.UUIDToString(ap.WorkspaceID),
		ActorType:   "agent",
		ActorID:     util.UUIDToString(leader.ID),
		Payload: map[string]any{
			"issue": IssueToMapResolved(ctx, s.Queries, issue, prefix),
		},
	})
	s.captureIssueCreatedFromAutomation(ap, run, issue, leader.ID)

	// The issue:created notification listener only handles handler.IssueResponse
	// payloads and only direct-notifies the assignee + @mentions; subscribers
	// don't get an inbox at creation time on the manual path because there are
	// none yet. The automation path is different: the template subscribers were
	// fanned out into issue_subscriber inside the tx above, so they exist at the
	// moment of creation and OQ3 says they should receive the same subscription
	// events as reason='manual'. Issue creation is one such event — so write
	// the inbox rows directly here. Done after commit so a failure here doesn't
	// roll back the issue itself.
	s.notifyAutomationSubscribersOnCreate(ctx, ap, issue, leader.ID, templateSubs)

	// Enqueue agent task via the existing flow. Team-assigned automations
	// route to the resolved leader as the executing agent (Path A from
	// MUL-2429); agent-assigned automations go through the standard issue
	// path. Both code paths land in agent_task_queue with agent_id = leader.
	// A MANUAL trigger (valid actorUserID) is a direct human action: enqueue via the
	// actor-carrying entry points so attribution resolves direct_human to the
	// triggering member (originator == accountable == actor, MUL-4302 §4). Schedule /
	// webhook dispatch has no actor and takes the plain entry points, where the
	// automation-origin issue resolves to rule_owner. The *WithHandoff variants are
	// the existing actor-carrying enqueue methods; the handoff note is empty here.
	if ap.ExecutorType == "team" {
		// Fail-closed invocation gate: verify the admission principal (manual
		// clicker, else creator — see automationAdmitInvoke) may still invoke the
		// leader. Catches configs that predate the save-time gate, and configs
		// that no longer pass (MUL-3963 / MUL-4525).
		if !s.automationAdmitInvoke(ctx, ap, leader, actorUserID) {
			return fmt.Errorf("not allowed to invoke private team leader")
		}
		if actorUserID.Valid {
			if _, err := s.TaskSvc.EnqueueTaskForTeamLeaderWithHandoff(ctx, issue, leader.ID, ap.ExecutorID, "", actorUserID); err != nil {
				return fmt.Errorf("enqueue team leader task: %w", err)
			}
		} else if _, err := s.TaskSvc.EnqueueTaskForTeamLeader(ctx, issue, leader.ID, ap.ExecutorID, pgtype.UUID{}); err != nil {
			return fmt.Errorf("enqueue team leader task: %w", err)
		}
	} else if actorUserID.Valid {
		if _, err := s.TaskSvc.EnqueueTaskForIssueWithHandoff(ctx, issue, "", actorUserID); err != nil {
			return fmt.Errorf("enqueue task for issue: %w", err)
		}
	} else if _, err := s.TaskSvc.EnqueueTaskForIssue(ctx, issue); err != nil {
		return fmt.Errorf("enqueue task for issue: %w", err)
	}

	slog.Info("automation dispatched (create_issue)",
		"automation_id", util.UUIDToString(ap.ID),
		"executor_type", ap.ExecutorType,
		"issue_id", util.UUIDToString(issue.ID),
		"leader_id", util.UUIDToString(leader.ID),
		"run_id", util.UUIDToString(run.ID),
	)
	return nil
}

// notifyAutomationSubscribersOnCreate writes an inbox_item for each template
// subscriber of an automation-created issue and broadcasts an inbox:new event
// so the recipient's inbox updates in real time. Mirrors the inbox payload
// shape from notification_listeners.go so the WS consumer sees the same fields
// the listener-driven path produces. Failures are logged, not propagated:
// the issue and its subscriber rows are already committed, and an inbox-write
// hiccup must not bubble up as a dispatch failure.
func (s *AutomationService) notifyAutomationSubscribersOnCreate(
	ctx context.Context,
	ap db.Automation,
	issue db.Issue,
	leaderID pgtype.UUID,
	subscribers []db.AutomationSubscriber,
) {
	if len(subscribers) == 0 {
		return
	}
	details, _ := json.Marshal(map[string]string{
		"automation_id": util.UUIDToString(ap.ID),
		"reason":       "automation",
	})
	for _, sub := range subscribers {
		// Automation subscribers are restricted to user_type='member' at the
		// handler boundary; defend in case that constraint is ever relaxed
		// (agents don't have inbox).
		if sub.UserType != "member" {
			continue
		}
		item, err := s.Queries.CreateInboxItem(ctx, db.CreateInboxItemParams{
			ID:            dbid.NewV7(),
			WorkspaceID:   ap.WorkspaceID,
			RecipientType: "member",
			RecipientID:   sub.UserID,
			Type:          "issue_subscribed",
			Severity:      "info",
			IssueID:       issue.ID,
			Title:         issue.Title,
			Body:          pgtype.Text{},
			ActorType:     pgtype.Text{String: "agent", Valid: true},
			ActorID:       leaderID,
			Details:       details,
		})
		if err != nil {
			slog.Error("automation subscriber inbox write failed",
				"automation_id", util.UUIDToString(ap.ID),
				"issue_id", util.UUIDToString(issue.ID),
				"recipient_id", util.UUIDToString(sub.UserID),
				"error", err,
			)
			continue
		}
		s.Bus.Publish(events.Event{
			Type:        protocol.EventInboxNew,
			WorkspaceID: util.UUIDToString(ap.WorkspaceID),
			ActorType:   "agent",
			ActorID:     util.UUIDToString(leaderID),
			Payload: map[string]any{
				"item": map[string]any{
					"id":             util.UUIDToString(item.ID),
					"workspace_id":   util.UUIDToString(item.WorkspaceID),
					"recipient_type": item.RecipientType,
					"recipient_id":   util.UUIDToString(item.RecipientID),
					"type":           item.Type,
					"severity":       item.Severity,
					"issue_id":       util.UUIDToPtr(item.IssueID),
					"issue_status":   issue.Status,
					"title":          item.Title,
					"body":           util.TextToPtr(item.Body),
					"read":           item.Read,
					"archived":       item.Archived,
					"created_at":     util.TimestampToString(item.CreatedAt),
					"actor_type":     util.TextToPtr(item.ActorType),
					"actor_id":       util.UUIDToPtr(item.ActorID),
					"details":        json.RawMessage(item.Details),
				},
			},
		})
	}
}

// errDispatchSkipped wraps a readiness failure encountered after the
// admission gate has already passed. dispatchRunOnly returns this when a
// resolved leader has gone offline / been archived between admission and
// task creation; DispatchAutomation recognises it and records a `skipped`
// run (with the wrapped reason) instead of a `failed` run.
//
// Without the sentinel, the existing failRun path would mark these races as
// failures and bubble a 500 out of the manual-trigger handler — both wrong
// (the work was never attempted, no one is at fault) and noisy (the failure
// monitor would auto-pause automations whose only crime was a flaky runtime).
type errDispatchSkipped struct {
	reason string
	// code is the stable, typed admission reason decided at THIS branch and
	// carried through to the response (MUL-4525) — never reverse-engineered from
	// the human-readable reason string above.
	code dispatch.ReasonCode
}

func (e *errDispatchSkipped) Error() string { return e.reason }

// dispatchRunOnly enqueues a direct agent task without creating an issue.
//
// For team automations, the executing agent is the team leader resolved at
// trigger time (Path A from MUL-2429). The same archived / runtime-bound /
// runtime-online gates that the upstream admission check (shouldSkipDispatch)
// applies also run here as belt-and-braces: if the leader changed between
// admission and dispatch, or the runtime went offline in the gap, we still
// fail closed instead of enqueueing a doomed task.
func (s *AutomationService) dispatchRunOnly(ctx context.Context, ap db.Automation, run *db.AutomationRun, actorUserID pgtype.UUID) error {
	agent, _, err := s.resolveAutomationLeader(ctx, ap)
	if err != nil {
		// Same admission-vs-failure classification as shouldSkipDispatch:
		// if the row disappeared or the team was archived between
		// admission and dispatch, that is a skip, not a failure.
		if errors.Is(err, pgx.ErrNoRows) || errors.Is(err, errTeamArchived) {
			return &errDispatchSkipped{reason: formatAdmissionReason(ap, "assignee no longer resolvable"), code: dispatch.ReasonTargetUnavailable}
		}
		return fmt.Errorf("resolve leader: %w", err)
	}
	verdict, err := AgentReadiness(ctx, s.runtimeLookup(), agent)
	if err != nil {
		return fmt.Errorf("check agent readiness: %w", err)
	}
	if !verdict.Ready() {
		return &errDispatchSkipped{reason: formatAdmissionReason(ap, verdict.Detail), code: verdict.Reason}
	}

	// Fail-closed invocation gate for team automations (admission principal =
	// manual clicker, else creator — see automationAdmitInvoke).
	if ap.ExecutorType == "team" && !s.automationAdmitInvoke(ctx, ap, agent, actorUserID) {
		return &errDispatchSkipped{reason: formatAdmissionReason(ap, "not allowed to invoke private team leader"), code: dispatch.ReasonInvocationNotAllowed}
	}

	// Attribution splits on the trigger. A MANUAL trigger is a direct human action:
	// the triggering member is direct_human and becomes BOTH originator (so the run
	// carries their authorization context) and accountable (MUL-4302 §4). A
	// schedule / webhook trigger has no human — originator_user_id stays NULL and
	// the audit-accountable human is the member currently RESPONSIBLE for the firing
	// trigger's effective config (its creator, then whoever last substantively edited
	// it) — trigger_owner, resolved from run.TriggerID (MUL-4302; Elon must-fix) —
	// degrading to the rule version publisher (rule_owner) when no such member is
	// recoverable, then to unattributed. Either way evidence points at the automation
	// run and the row is never a NULL-source bypass.
	var automationAttr attribution.Result
	if actorUserID.Valid {
		automationAttr = attribution.DirectHumanRun(actorUserID, attribution.EvidenceAutomationRun, run.ID)
	} else {
		automationAttr = triggerOwnerAttribution(ctx, s.Queries, run.TriggerID, ap.WorkspaceID, ap.ID, attribution.EvidenceAutomationRun, run.ID)
	}
	// If no precise human resolved (a version-less automation), degrade to
	// owner_fallback (accountable = agent owner), or skip the dispatch when the
	// workspace is fail-closed (MUL-4302 §3.5).
	automationAttr, err = s.TaskSvc.applyAttributionFallback(ctx, automationAttr, agent)
	if err != nil {
		return &errDispatchSkipped{reason: formatAdmissionReason(ap, "workspace fail-closed: no accountable human for automation run"), code: dispatch.ReasonAttributionBlocked}
	}
	apSource, _, apEvidenceKind, apEvidenceRef := attributionCreateParams(automationAttr)
	task, err := s.Queries.CreateAutomationTask(ctx, db.CreateAutomationTaskParams{
		ID:             dbid.NewV7(),
		AgentID:        agent.ID,
		RuntimeID:      agent.RuntimeID,
		Priority:       0,
		AutomationRunID: run.ID,
		// Snapshot the automation title so task rows self-describe later
		// without joining back to automation. Truncated for the same
		// transmission-cost reason as comment-driven summaries.
		TriggerSummary: pgtype.Text{
			String: truncateForSummary(ap.Title, triggerSummaryMaxLen),
			Valid:  ap.Title != "",
		},
		OriginatorUserID:     automationAttr.UserID,
		AccountableUserID:    automationAttr.AccountableUserID,
		RuleVersionID:        automationAttr.RuleVersionID,
		OriginatorSource:     apSource,
		TriggerEvidenceKind:  apEvidenceKind,
		TriggerEvidenceRefID: apEvidenceRef,
	})
	if err != nil {
		return fmt.Errorf("create automation task: %w", err)
	}

	// Update run with task reference.
	updatedRun, err := s.Queries.UpdateAutomationRunRunning(ctx, db.UpdateAutomationRunRunningParams{
		ID:     run.ID,
		TaskID: task.ID,
	})
	if err != nil {
		slog.Warn("failed to update run with task_id", "run_id", util.UUIDToString(run.ID), "error", err)
	} else {
		*run = updatedRun
	}

	// Drop the empty-claim cache and wake the daemon. dispatchRunOnly
	// inserts the task row directly via Queries.CreateAutomationTask
	// (bypassing TaskService.Enqueue*), so without this the runtime
	// would not get a wakeup and any cached "empty" verdict would
	// stall the task until the TTL expired.
	s.TaskSvc.NotifyTaskEnqueued(ctx, task)

	slog.Info("automation dispatched (run_only)",
		"automation_id", util.UUIDToString(ap.ID),
		"task_id", util.UUIDToString(task.ID),
		"run_id", util.UUIDToString(run.ID),
	)
	return nil
}

// SyncRunFromIssue updates the automation run when its linked issue reaches a terminal status.
func (s *AutomationService) SyncRunFromIssue(ctx context.Context, issue db.Issue) {
	if !issue.OriginType.Valid || issue.OriginType.String != "automation" {
		return
	}

	run, err := s.Queries.GetAutomationRunByIssue(ctx, issue.ID)
	if err != nil {
		return // no active run linked to this issue
	}
	automation, err := s.Queries.GetAutomation(ctx, run.AutomationID)
	if err != nil {
		return
	}

	wsID := util.UUIDToString(issue.WorkspaceID)

	// A custom status finalizes the run exactly like the canonical status it
	// inherits. Built-in keys resolve to themselves without a query, so this
	// is a no-op for every workspace that has not defined a custom status.
	// The failure reason below deliberately keeps issue.Status, not the
	// normalized key, so the audit trail names the status a human actually
	// chose. (MUL-6243)
	effectiveStatus := issuestatus.Effective(ctx, s.Queries, issue.WorkspaceID, issue.Status)

	switch effectiveStatus {
	case "done", "in_review":
		updatedRun, err := s.completeAutomationRun(ctx, db.UpdateAutomationRunCompletedParams{
			ID: run.ID,
		})
		if err != nil {
			slog.Warn("failed to complete automation run", "run_id", util.UUIDToString(run.ID), "error", err)
			return
		}
		s.captureAutomationRunCompleted(automation, updatedRun)
		s.publishRunDone(wsID, updatedRun, "completed")
	case "cancelled", "blocked":
		reason := "issue " + issue.Status
		updatedRun, err := s.failAutomationRun(ctx, db.UpdateAutomationRunFailedParams{
			ID:            run.ID,
			FailureReason: pgtype.Text{String: reason, Valid: true},
		})
		if err != nil {
			slog.Warn("failed to fail automation run", "run_id", util.UUIDToString(run.ID), "error", err)
			return
		}
		s.captureAutomationRunFailed(automation, updatedRun, updatedRun.Source, reason)
		s.publishRunDone(wsID, updatedRun, "failed")
	}
}

// SyncRunFromTask updates the automation run when a run_only task completes or fails.
func (s *AutomationService) SyncRunFromTask(ctx context.Context, task db.AgentTaskQueue) {
	if !task.AutomationRunID.Valid {
		return
	}

	run, err := s.Queries.GetAutomationRun(ctx, task.AutomationRunID)
	if err != nil {
		return
	}

	automation, err := s.Queries.GetAutomation(ctx, run.AutomationID)
	if err != nil {
		return
	}
	wsID := util.UUIDToString(automation.WorkspaceID)

	switch task.Status {
	case "completed":
		updatedRun, err := s.completeAutomationRun(ctx, db.UpdateAutomationRunCompletedParams{
			ID:     run.ID,
			Result: task.Result,
		})
		if err != nil {
			slog.Warn("failed to complete automation run from task", "run_id", util.UUIDToString(run.ID), "error", err)
			return
		}
		s.captureAutomationRunCompleted(automation, updatedRun)
		s.publishRunDone(wsID, updatedRun, "completed")
	case "failed", "cancelled":
		reason := "task " + task.Status
		if task.Error.Valid {
			reason = task.Error.String
		}
		updatedRun, err := s.failAutomationRun(ctx, db.UpdateAutomationRunFailedParams{
			ID:            run.ID,
			FailureReason: pgtype.Text{String: reason, Valid: true},
		})
		if err != nil {
			slog.Warn("failed to fail automation run from task", "run_id", util.UUIDToString(run.ID), "error", err)
			return
		}
		s.captureAutomationRunFailed(automation, updatedRun, updatedRun.Source, reason)
		s.publishRunDone(wsID, updatedRun, "failed")
	}
}

// SyncRunFromLinkedIssueTask fails a create_issue automation run when its
// linked issue task fails terminally before the issue itself reaches a
// terminal status. create_issue tasks are linked through issue_id rather than
// automation_run_id, so SyncRunFromTask cannot see them directly. Without this
// the run would hang in `issue_created` forever — and because the failure-rate
// auto-pause monitor excludes issue_created/running runs, a consistently
// failing automation would never trip the auto-pause either.
//
// "Terminal" means no task is still active for the issue. FailTask enqueues an
// auto-retry for infra-shaped failures (timeout, runtime offline/recovery,
// codex no-progress) BEFORE it broadcasts the failure event, so an active task
// here means another attempt is already in flight — we wait for it instead of
// failing the run prematurely. Once retries are exhausted (or the failure was
// never retryable in the first place), the run fails carrying the task's reason.
func (s *AutomationService) SyncRunFromLinkedIssueTask(ctx context.Context, task db.AgentTaskQueue) {
	if task.AutomationRunID.Valid || !task.IssueID.Valid || task.Status != "failed" {
		return
	}
	// Only create_issue runs link through issue_id (and their linked issue is
	// always origin_type=automation by construction), so a hit here both
	// identifies an in-flight create_issue run and bails the common case of
	// ordinary issue/chat task failures after a single query.
	run, err := s.Queries.GetAutomationRunByIssue(ctx, task.IssueID)
	if err != nil {
		return // no active run linked to this issue
	}
	// A still-active task — typically the auto-retry FailTask just enqueued —
	// means the dispatch isn't terminal yet; wait for the final attempt.
	hasActive, err := s.Queries.HasActiveTaskForIssue(ctx, task.IssueID)
	if err != nil {
		slog.Warn("failed to check active tasks for automation issue failure",
			"issue_id", util.UUIDToString(task.IssueID),
			"task_id", util.UUIDToString(task.ID),
			"error", err,
		)
		return
	}
	if hasActive {
		return
	}
	automation, err := s.Queries.GetAutomation(ctx, run.AutomationID)
	if err != nil {
		return
	}

	reason := taskFailureReasonForAutomationRun(task)
	updatedRun, err := s.failAutomationRun(ctx, db.UpdateAutomationRunFailedParams{
		ID:            run.ID,
		FailureReason: pgtype.Text{String: reason, Valid: reason != ""},
	})
	if err != nil {
		slog.Warn("failed to fail automation run from linked issue task",
			"run_id", util.UUIDToString(run.ID),
			"issue_id", util.UUIDToString(task.IssueID),
			"task_id", util.UUIDToString(task.ID),
			"error", err,
		)
		return
	}
	s.captureAutomationRunFailed(automation, updatedRun, updatedRun.Source, reason)
	s.publishRunDone(util.UUIDToString(automation.WorkspaceID), updatedRun, "failed")
}

func taskFailureReasonForAutomationRun(task db.AgentTaskQueue) string {
	if task.Error.Valid && strings.TrimSpace(task.Error.String) != "" {
		return task.Error.String
	}
	if task.FailureReason.Valid && strings.TrimSpace(task.FailureReason.String) != "" {
		return task.FailureReason.String
	}
	return "task failed"
}

// handleDispatchSkip recognises an errDispatchSkipped returned from a
// dispatch function and rewrites the in-flight run to `skipped` (instead of
// `failed`). Returns the updated run on a real skip, nil otherwise — callers
// fall through to the failure path on nil.
//
// Lives here, not inside dispatchRunOnly, because the run row was created by
// DispatchAutomation up the stack and the failure-vs-skip distinction is
// owned by the dispatcher entry point. Keeps dispatchRunOnly free of
// state-mutation helpers.
func (s *AutomationService) handleDispatchSkip(ctx context.Context, ap db.Automation, run *db.AutomationRun, err error) (*db.AutomationRun, dispatch.ReasonCode) {
	var skipErr *errDispatchSkipped
	if !errors.As(err, &skipErr) {
		return nil, ""
	}
	updated, uerr := s.skipAutomationRun(ctx, db.UpdateAutomationRunSkippedParams{
		ID:            run.ID,
		FailureReason: pgtype.Text{String: skipErr.reason, Valid: true},
		ReasonCode:    pgtype.Text{String: string(skipErr.code), Valid: skipErr.code != ""},
	})
	if uerr != nil {
		slog.Warn("failed to mark dispatch as skipped",
			"run_id", util.UUIDToString(run.ID), "error", uerr)
		// Leave the run in its current (running/issue_created) state if
		// the update failed; the failure monitor will eventually fail it
		// out, but at least we didn't pretend it succeeded.
		return nil, ""
	}
	*run = updated
	slog.Info("automation dispatch skipped post-admission",
		"automation_id", util.UUIDToString(ap.ID),
		"run_id", util.UUIDToString(run.ID),
		"reason", skipErr.reason,
	)
	// Bump last_run_at on parity with recordSkippedRun (pre-flight skip) and
	// the success path: from the scheduler's / UI's point of view we did
	// evaluate the trigger this tick, even though the post-admission gate
	// caught a late readiness regression.
	s.Queries.UpdateAutomationLastRunAt(ctx, ap.ID)
	s.publishRunDone(util.UUIDToString(ap.WorkspaceID), updated, "skipped")
	return run, skipErr.code
}

func (s *AutomationService) failRun(ctx context.Context, runID pgtype.UUID, reason string) {
	if _, err := s.failAutomationRun(ctx, db.UpdateAutomationRunFailedParams{
		ID:            runID,
		FailureReason: pgtype.Text{String: reason, Valid: true},
		ReasonCode:    pgtype.Text{String: string(dispatch.ReasonInternalError), Valid: true},
	}); err != nil {
		slog.Warn("failed to mark automation run as failed", "run_id", util.UUIDToString(runID), "error", err)
	}
}

// shouldSkipDispatch is the pre-flight admission check from MUL-1899.
// Returns (reason, true) when dispatching now would only enqueue a doomed
// task — i.e. the assignee (or, for team automations, the team leader) is
// gone, archived, has no runtime bound, or its runtime is not currently
// online. Returns ("", false) on the happy path.
//
// Errors are split into two classes:
//   - pgx.ErrNoRows / errTeamArchived (the row truly doesn't exist or is
//     archived) → hard skip. Retrying won't change anything; piling failed
//     runs would pollute the failure-rate auto-pause monitor.
//   - Anything else (connection drop, statement timeout, etc.) → fail-open:
//     log + do not skip, so a transient DB hiccup never silently swallows a
//     scheduled run. Migration 096 removed the agent FK on automation, so an
//     agent assignee being missing is now a real condition the gate must
//     handle (previously cascade-deleted).
func (s *AutomationService) shouldSkipDispatch(ctx context.Context, ap db.Automation, actorUserID pgtype.UUID) (string, dispatch.ReasonCode, bool) {
	if !ap.ExecutorID.Valid {
		return "automation has no assignee", dispatch.ReasonTargetUnavailable, true
	}
	agent, teamResolved, err := s.resolveAutomationLeader(ctx, ap)
	if err != nil {
		// Hard-skip the cases where another retry will produce the same
		// outcome. Logging is unconditional so ops can still spot a run of
		// dangling rows pointing at a deleted agent / archived team.
		missing := errors.Is(err, pgx.ErrNoRows)
		archived := errors.Is(err, errTeamArchived)
		slog.Warn("automation admission: failed to resolve leader",
			"automation_id", util.UUIDToString(ap.ID),
			"executor_type", ap.ExecutorType,
			"executor_id", util.UUIDToString(ap.ExecutorID),
			"missing", missing,
			"archived", archived,
			"error", err,
		)
		switch {
		case archived:
			// Team row exists but is archived — DeleteTeam's transfer
			// should have rewritten this automation's assignee to the leader
			// already; surfacing the case explicitly keeps the failure
			// reason useful when something slipped past the transfer.
			return "assignee team is archived", dispatch.ReasonTargetUnavailable, true
		case missing && teamResolved:
			return "assignee team cannot be resolved", dispatch.ReasonTargetUnavailable, true
		case missing && !teamResolved:
			// Agent row gone. With migration 096 the FK is gone too, so
			// this is the new "agent was hard-deleted under us" case. Skip
			// rather than fail-open: we know retrying will not help.
			return "assignee agent no longer exists", dispatch.ReasonTargetUnavailable, true
		}
		// Transient DB error — fail-open so the next scheduler tick gets a
		// chance to succeed.
		return "", "", false
	}
	verdict, err := AgentReadiness(ctx, s.runtimeLookup(), agent)
	if err != nil {
		slog.Warn("automation admission: failed to load runtime",
			"automation_id", util.UUIDToString(ap.ID),
			"runtime_id", util.UUIDToString(agent.RuntimeID),
			"error", err,
		)
		return "", "", false
	}
	if !verdict.Ready() {
		// A merely-offline machine still gets create_issue work: the issue is
		// written server-side and the run waits for the laptop to come back. An
		// unusable runtime does not qualify — nothing there can run until a
		// human repairs it, so a doomed issue-create is not an improvement.
		if ap.ExecutionMode == "create_issue" && verdict.Availability == AgentWaitable {
			slog.Info("automation admission: allowing create_issue dispatch for offline runtime",
				"automation_id", util.UUIDToString(ap.ID),
				"runtime_id", util.UUIDToString(agent.RuntimeID),
				"reason", verdict.Detail,
			)
		} else {
			return formatAdmissionReason(ap, verdict.Detail), verdict.Reason, true
		}
	}
	// Invocation gate at the automation layer (MUL-3963 / MUL-4525). The
	// admission principal depends on how the dispatch was triggered: a MANUAL
	// "run now" (actorUserID valid) is a direct human action gated by the
	// current CLICKER's access — not the automation creator's — so admission and
	// attribution credit the same member and never fork. Automation (schedule /
	// webhook / api, actorUserID invalid) has no human in the loop and falls
	// back to the creator. Admins do NOT bypass a private agent they do not own;
	// agent-created automations are judged as workspace principals. For team
	// automations the gate runs against the resolved leader.
	if !s.automationAdmitInvoke(ctx, ap, agent, actorUserID) {
		if actorUserID.Valid {
			return "you are not allowed to trigger this automation's assignee agent", dispatch.ReasonInvocationNotAllowed, true
		}
		return "automation creator lacks access to private assignee agent", dispatch.ReasonInvocationNotAllowed, true
	}
	return "", "", false
}

// formatAdmissionReason rewrites the generic AgentReadiness reason into the
// admission-gate phrasing the failure monitor and existing alerting are tuned
// for. Keeping the prefix stable matters: dashboards group skip reasons by
// substring ("offline at dispatch time" is how the MUL-1899 alert fires).
//
// For team automations the message names the team so an operator looking at
// the failure_reason field knows which team's leader is down without
// joining back to automation_run.team_id.
func formatAdmissionReason(ap db.Automation, raw string) string {
	prefix := "assignee "
	if ap.ExecutorType == "team" {
		prefix = "team leader "
	}
	switch raw {
	case "agent is archived":
		return prefix + "agent is archived"
	case "agent has no runtime bound":
		return prefix + "agent has no runtime bound"
	default:
		// raw is "agent runtime is X" — surface the runtime status while
		// preserving the legacy "at dispatch time" suffix from MUL-1899
		// so alert queries do not need to change.
		return raw + " at dispatch time"
	}
}

// errTeamArchived signals that an automation's team assignee has been
// archived. Distinct from a missing/loadable-but-failed team so the
// admission gate can phrase the skip reason precisely and the failure
// monitor does not see "cannot be resolved" wear noise for what is a
// known, expected post-archive condition.
var errTeamArchived = errors.New("team is archived")

// resolveAutomationLeader returns the agent that will actually execute the
// automation's work. For executor_type='agent' the agent is the assignee
// itself; for executor_type='team' it is the team's leader_id. The second
// return is true when the resolver took the team branch — callers use this
// to distinguish "failed loading an agent" from "failed loading a team", so
// the admission gate can choose between fail-open (transient DB error on a
// known-good agent) and fail-closed (team row gone, no point retrying).
//
// Archived teams are rejected here too: TransferTeamAutomationsToLeader
// flips surviving automations to executor_type='agent' on DeleteTeam, but
// the gate still has to fail closed for any row that slips through that
// transfer (e.g. team archived through a code path that bypasses the
// handler) so an archived team never produces work.
//
// Unknown executor_type values return an error. executor_type is gated by a
// CHECK constraint at the DB layer, so this only fires if a future code path
// inserts a row that bypasses the check.
func (s *AutomationService) resolveAutomationLeader(ctx context.Context, ap db.Automation) (agent db.Agent, teamResolved bool, err error) {
	switch ap.ExecutorType {
	case "", "agent":
		agent, err = s.Queries.GetAgent(ctx, ap.ExecutorID)
		return agent, false, err
	case "team":
		team, err := s.Queries.GetTeam(ctx, ap.ExecutorID)
		if err != nil {
			return db.Agent{}, true, fmt.Errorf("load team: %w", err)
		}
		if team.ArchivedAt.Valid {
			return db.Agent{}, true, errTeamArchived
		}
		agent, err = s.Queries.GetAgent(ctx, team.LeaderID)
		if err != nil {
			return db.Agent{}, true, fmt.Errorf("load team leader: %w", err)
		}
		return agent, true, nil
	default:
		return db.Agent{}, false, fmt.Errorf("unknown executor_type %q", ap.ExecutorType)
	}
}

// automationTeamAttribution returns the team_id attribution hook for an
// automation_run row. Only populated when executor_type='team'. First-version
// reports do not consume this; it exists so a future team-cost view does not
// need to backfill — see RFC §4.e (MUL-2429).
func automationTeamAttribution(ap db.Automation) pgtype.UUID {
	if ap.ExecutorType == "team" && ap.ExecutorID.Valid {
		return ap.ExecutorID
	}
	return pgtype.UUID{}
}

// recordSkippedRun persists a `skipped` automation_run with the given reason
// and emits the same WS / analytics signals that a normal terminal transition
// would. Returns the run + nil error so callers (scheduler tick, manual
// trigger handler) treat this as a successful — but no-op — dispatch.
func (s *AutomationService) recordSkippedRun(
	ctx context.Context,
	automation db.Automation,
	triggerID pgtype.UUID,
	source string,
	payload []byte,
	plannedAt pgtype.Timestamptz,
	webhookDeliveryID pgtype.UUID,
	reason string,
	reasonCode ...dispatch.ReasonCode,
) (*db.AutomationRun, error) {
	code := pgtype.Text{}
	if len(reasonCode) > 0 && reasonCode[0] != "" {
		code = pgtype.Text{String: string(reasonCode[0]), Valid: true}
	}
	run, err := s.Queries.CreateAutomationRun(ctx, db.CreateAutomationRunParams{
		ID:                dbid.NewV7(),
		AutomationID:       automation.ID,
		TriggerID:         triggerID,
		Source:            source,
		Status:            "skipped",
		TriggerPayload:    payload,
		TeamID:           automationTeamAttribution(automation),
		PlannedAt:         plannedAt,
		WebhookDeliveryID: webhookDeliveryID,
		ReasonCode:        code,
	})
	if err != nil {
		return nil, fmt.Errorf("create skipped run: %w", err)
	}

	updated, err := s.Queries.UpdateAutomationRunSkipped(ctx, db.UpdateAutomationRunSkippedParams{
		ID:            run.ID,
		FailureReason: pgtype.Text{String: reason, Valid: true},
		ReasonCode:    code,
	})
	if err == nil {
		run = updated
	} else {
		slog.Warn("failed to set skip reason on automation run",
			"run_id", util.UUIDToString(run.ID), "error", err)
	}

	slog.Info("automation dispatch skipped",
		"automation_id", util.UUIDToString(automation.ID),
		"run_id", util.UUIDToString(run.ID),
		"source", source,
		"reason", reason,
	)

	// Bump last_run_at so scheduler advancement and "last seen" UI both
	// reflect that we did evaluate the trigger this tick.
	s.Queries.UpdateAutomationLastRunAt(ctx, automation.ID)

	s.publishRunDone(util.UUIDToString(automation.WorkspaceID), run, "skipped")
	return &run, nil
}

func (s *AutomationService) publishRunDone(workspaceID string, run db.AutomationRun, status string) {
	s.Bus.Publish(events.Event{
		Type:        protocol.EventAutomationRunDone,
		WorkspaceID: workspaceID,
		ActorType:   "system",
		Payload: map[string]any{
			"run_id":       util.UUIDToString(run.ID),
			"automation_id": util.UUIDToString(run.AutomationID),
			"status":       status,
		},
	})
}

func (s *AutomationService) captureIssueCreatedFromAutomation(ap db.Automation, run *db.AutomationRun, issue db.Issue, leaderID pgtype.UUID) {
	if s.TaskSvc == nil || s.TaskSvc.Analytics == nil {
		return
	}
	// For PostHog the agent_id should be the agent that will actually run
	// the work (the resolved leader for team automations) so per-agent task
	// counts line up with what daemons report.
	obsmetrics.RecordEvent(s.TaskSvc.Analytics, s.TaskSvc.Metrics, analytics.IssueCreated(
		automationActorID(ap),
		util.UUIDToString(ap.WorkspaceID),
		util.UUIDToString(issue.ID),
		util.UUIDToString(leaderID),
		"",
		util.UUIDToString(run.ID),
		analytics.SourceAutomation,
		analytics.PlatformServer,
	))
}

func (s *AutomationService) captureAutomationRunStarted(ap db.Automation, run db.AutomationRun, triggerSource string) {
	if s.TaskSvc == nil || s.TaskSvc.Analytics == nil {
		return
	}
	obsmetrics.RecordEvent(s.TaskSvc.Analytics, s.TaskSvc.Metrics, analytics.AutomationRunStarted(
		automationActorID(ap),
		util.UUIDToString(ap.WorkspaceID),
		util.UUIDToString(ap.ID),
		util.UUIDToString(run.ID),
		triggerSource, // cadence proxy: see automation cadence note in metrics/labels_pr3.go
		s.automationAssigneeAnalytics(ap),
		triggerSource,
	))
}

func (s *AutomationService) captureAutomationRunCompleted(ap db.Automation, run db.AutomationRun) {
	if s.TaskSvc == nil || s.TaskSvc.Analytics == nil {
		return
	}
	obsmetrics.RecordEvent(s.TaskSvc.Analytics, s.TaskSvc.Metrics, analytics.AutomationRunCompleted(
		automationActorID(ap),
		util.UUIDToString(ap.WorkspaceID),
		util.UUIDToString(ap.ID),
		util.UUIDToString(run.ID),
		run.Source,
		s.automationAssigneeAnalytics(ap),
		run.Source,
		automationRunDurationMS(run),
	))
}

func (s *AutomationService) captureAutomationRunFailed(ap db.Automation, run db.AutomationRun, triggerSource, reason string) {
	if s.TaskSvc == nil || s.TaskSvc.Analytics == nil {
		return
	}
	if reason == "" {
		reason = "unknown"
	}
	obsmetrics.RecordEvent(s.TaskSvc.Analytics, s.TaskSvc.Metrics, analytics.AutomationRunFailed(
		automationActorID(ap),
		util.UUIDToString(ap.WorkspaceID),
		util.UUIDToString(ap.ID),
		util.UUIDToString(run.ID),
		triggerSource,
		s.automationAssigneeAnalytics(ap),
		triggerSource,
		reason,
		automationErrorType(reason),
		false,
		automationRunDurationMS(run),
	))
}

// automationAssigneeAnalytics builds the PostHog assignee descriptor for an
// automation. For team automations agent_id is best-effort the resolved
// leader (so per-agent funnels stay consistent); a resolve error degrades
// to the raw executor_id rather than dropping the event — incomplete data
// in the dashboard is preferable to silent attribution gaps.
func (s *AutomationService) automationAssigneeAnalytics(ap db.Automation) analytics.AutomationAssignee {
	assignee := analytics.AutomationAssignee{
		ExecutorType: ap.ExecutorType,
	}
	if ap.ExecutorType == "team" {
		assignee.TeamID = util.UUIDToString(ap.ExecutorID)
		if leader, _, err := s.resolveAutomationLeader(context.Background(), ap); err == nil {
			assignee.AgentID = util.UUIDToString(leader.ID)
		} else {
			assignee.AgentID = util.UUIDToString(ap.ExecutorID)
		}
	} else {
		assignee.AgentID = util.UUIDToString(ap.ExecutorID)
	}
	return assignee
}

func automationErrorType(reason string) string {
	switch {
	case strings.Contains(reason, "unknown execution_mode"):
		return "configuration"
	case strings.HasPrefix(reason, "issue "):
		return "issue_terminal"
	case strings.Contains(reason, "create issue"), strings.Contains(reason, "enqueue task"), strings.Contains(reason, "dispatch"):
		return "dispatch_error"
	case strings.HasPrefix(reason, "task "):
		return "task_error"
	default:
		return "automation_error"
	}
}

func automationActorID(ap db.Automation) string {
	id := util.UUIDToString(ap.CreatedByID)
	if ap.CreatedByType == "agent" && id != "" {
		return "agent:" + id
	}
	if id != "" {
		return id
	}
	return "system"
}

func automationRunDurationMS(run db.AutomationRun) int64 {
	if !run.CompletedAt.Valid {
		return 0
	}
	start := run.TriggeredAt
	if !start.Valid {
		start = run.CreatedAt
	}
	if !start.Valid {
		return 0
	}
	ms := run.CompletedAt.Time.Sub(start.Time).Milliseconds()
	if ms < 0 {
		return 0
	}
	return ms
}

func (s *AutomationService) resolveAutomationTriggerTimezone(ctx context.Context, triggerID pgtype.UUID) string {
	if !triggerID.Valid || s == nil || s.Queries == nil {
		return DefaultAutomationTriggerTimezone
	}

	trigger, err := s.Queries.GetAutomationTrigger(ctx, triggerID)
	if err != nil {
		slog.Warn("failed to load automation trigger timezone; falling back to UTC",
			"trigger_id", util.UUIDToString(triggerID),
			"error", err,
		)
		return DefaultAutomationTriggerTimezone
	}

	timezone := strings.TrimSpace(trigger.Timezone.String)
	if !trigger.Timezone.Valid || timezone == "" {
		return DefaultAutomationTriggerTimezone
	}
	if _, err := time.LoadLocation(timezone); err != nil {
		slog.Warn("invalid automation trigger timezone; falling back to UTC",
			"trigger_id", util.UUIDToString(triggerID),
			"timezone", timezone,
			"error", err,
		)
		return DefaultAutomationTriggerTimezone
	}
	return timezone
}

func formatAutomationRunTimestamp(run db.AutomationRun, timezone string) string {
	triggeredAt := automationRunTriggeredAt(run)
	loc, label := automationTriggerLocation(timezone)
	return triggeredAt.In(loc).Format("2006-01-02 15:04") + " " + label
}

func formatAutomationRunDate(run db.AutomationRun, timezone string) string {
	triggeredAt := automationRunTriggeredAt(run)
	loc, _ := automationTriggerLocation(timezone)
	return triggeredAt.In(loc).Format("2006-01-02")
}

func automationRunTriggeredAt(run db.AutomationRun) time.Time {
	if run.TriggeredAt.Valid {
		return run.TriggeredAt.Time
	}
	if run.CreatedAt.Valid {
		return run.CreatedAt.Time
	}
	return time.Now().UTC()
}

func automationTriggerLocation(timezone string) (*time.Location, string) {
	label := strings.TrimSpace(timezone)
	if label == "" {
		label = DefaultAutomationTriggerTimezone
	}
	loc, err := time.LoadLocation(label)
	if err != nil {
		return time.UTC, DefaultAutomationTriggerTimezone
	}
	return loc, label
}

// buildIssueDescription appends an automation system instruction to the
// user-provided description, asking the agent to rename the issue after
// it understands the actual work. For webhook-sourced runs, also appends
// a payload section so the agent has the event context inline (otherwise
// the agent only sees the issue body, never the run's trigger_payload).
func (s *AutomationService) buildIssueDescription(ap db.Automation, run db.AutomationRun, triggerTimezone string) pgtype.Text {
	triggeredAt := formatAutomationRunTimestamp(run, triggerTimezone)
	var b strings.Builder
	b.WriteString(ap.Description.String)
	b.WriteString("\n\n---\n*Automation run triggered at ")
	b.WriteString(triggeredAt)
	b.WriteString(". After starting work, rename this issue to accurately reflect what you are doing.*")

	if run.Source == "webhook" && len(run.TriggerPayload) > 0 {
		event := "webhook.received"
		var payloadJSON []byte
		var env struct {
			Event        string          `json:"event"`
			EventPayload json.RawMessage `json:"eventPayload"`
		}
		if err := json.Unmarshal(run.TriggerPayload, &env); err == nil {
			if env.Event != "" {
				event = env.Event
			}
			if len(env.EventPayload) > 0 {
				if pretty, err := prettifyJSON(env.EventPayload); err == nil {
					payloadJSON = pretty
				}
			}
		}
		if len(payloadJSON) == 0 {
			if pretty, err := prettifyJSON(run.TriggerPayload); err == nil {
				payloadJSON = pretty
			} else {
				payloadJSON = run.TriggerPayload
			}
		}
		b.WriteString("\n\nWebhook event: ")
		b.WriteString(event)
		b.WriteString("\n\nWebhook payload:\n```json\n")
		b.Write(payloadJSON)
		b.WriteString("\n```")
	}

	return pgtype.Text{String: b.String(), Valid: true}
}

func prettifyJSON(raw []byte) ([]byte, error) {
	var v any
	if err := json.Unmarshal(raw, &v); err != nil {
		return nil, err
	}
	return json.MarshalIndent(v, "", "  ")
}

// issueTitleTemplateTokenRE matches any {{...}} token in an issue-title
// template. We deliberately permit whitespace inside the braces ({{ date }})
// so users can format templates either way; the canonical token is still
// {{date}}.
var issueTitleTemplateTokenRE = regexp.MustCompile(`\{\{\s*([^{}]*?)\s*\}\}`)

// interpolateTemplate substitutes supported {{name}} placeholders in the
// issue title template. Whitespace inside the braces ({{ date }}) is
// tolerated so the render layer accepts every form that
// ValidateIssueTitleTemplate accepts — otherwise users would save templates
// that pass validation but still emit a literal token at trigger time.
func (s *AutomationService) interpolateTemplate(ap db.Automation, run db.AutomationRun, triggerTimezone string) string {
	tmpl := ap.Title
	if ap.IssueTitleTemplate.Valid && ap.IssueTitleTemplate.String != "" {
		tmpl = ap.IssueTitleTemplate.String
	}
	triggerDate := formatAutomationRunDate(run, triggerTimezone)
	return issueTitleTemplateTokenRE.ReplaceAllStringFunc(tmpl, func(match string) string {
		name := strings.TrimSpace(match[2 : len(match)-2])
		switch name {
		case "date":
			return triggerDate
		default:
			return match
		}
	})
}

// SupportedIssueTitleTemplateVariables enumerates the placeholders that
// interpolateTemplate will substitute. Keep this in sync with the
// substitution logic above and with the docs in automations.mdx /
// automations.zh.mdx.
var SupportedIssueTitleTemplateVariables = []string{"date"}

// ValidateIssueTitleTemplate rejects templates that contain any {{...}} token
// other than the supported set. An empty template is valid (the automation
// falls back to its own Title). The error message names the first offending
// token to keep CLI feedback actionable.
func ValidateIssueTitleTemplate(tmpl string) error {
	if tmpl == "" {
		return nil
	}
	for _, m := range issueTitleTemplateTokenRE.FindAllStringSubmatch(tmpl, -1) {
		name := m[1]
		if !isSupportedIssueTitleVariable(name) {
			return fmt.Errorf(
				"unknown template variable %q; supported: {{%s}}",
				name,
				strings.Join(SupportedIssueTitleTemplateVariables, "}}, {{"),
			)
		}
	}
	return nil
}

func isSupportedIssueTitleVariable(name string) bool {
	for _, v := range SupportedIssueTitleTemplateVariables {
		if name == v {
			return true
		}
	}
	return false
}

func (s *AutomationService) getIssuePrefix(workspaceID pgtype.UUID) string {
	ws, err := s.Queries.GetWorkspace(context.Background(), workspaceID)
	if err != nil {
		return ""
	}
	return ws.IssuePrefix
}

// canCreatorInvokeAgent checks whether the automation's creator may invoke the
// target agent under the invocation-permission model (MUL-3963). It mirrors
// handler.canInvokeAgent with the automation creator as the effective user:
//   - member creator who owns the agent -> always
//   - private agent -> only the owner (NO admin bypass, NO agent-created bypass)
//   - public_to agent -> workspace target admits any workspace-member creator
//     (and agent-created automations as workspace principals); member target
//     admits the matching creator; team targets are inert.
//
// Fail-closed on any lookup error.
// automationAdmitInvoke decides whether the dispatch's admission principal may
// invoke the target agent (MUL-4525). A MANUAL "run now" (actorUserID valid) is
// a direct human action gated by the CURRENT clicker's access, so admission and
// attribution credit the same member. Automation (schedule / webhook / api,
// actorUserID invalid) has no human in the loop and falls back to the automation
// creator. Both branches fail closed and never grant an admin bypass.
func (s *AutomationService) automationAdmitInvoke(ctx context.Context, ap db.Automation, agent db.Agent, actorUserID pgtype.UUID) bool {
	if actorUserID.Valid {
		return s.canMemberInvokeAgent(ctx, agent, actorUserID, ap.WorkspaceID)
	}
	return s.canCreatorInvokeAgent(ctx, ap, agent)
}

// canMemberInvokeAgent checks whether a specific member may invoke the agent
// under the invocation-permission model (MUL-3963). It mirrors
// handler.canInvokeAgent with a member effective user — used for a manual
// automation "run now" where the clicker, not the creator, is the admission
// principal. Fail-closed on any lookup error; no admin bypass.
func (s *AutomationService) canMemberInvokeAgent(ctx context.Context, agent db.Agent, memberUserID pgtype.UUID, workspaceID pgtype.UUID) bool {
	userID := util.UUIDToString(memberUserID)
	if userID == "" {
		return false
	}
	if util.UUIDToString(agent.OwnerID) == userID {
		return true
	}
	if agent.PermissionMode != "public_to" {
		return false
	}
	targets, err := s.Queries.ListAgentInvocationTargets(ctx, agent.ID)
	if err != nil {
		return false
	}
	isWorkspaceMember := false
	if _, err := s.Queries.GetMemberByUserAndWorkspace(ctx, db.GetMemberByUserAndWorkspaceParams{
		UserID:      memberUserID,
		WorkspaceID: workspaceID,
	}); err == nil {
		isWorkspaceMember = true
	}
	for _, t := range targets {
		switch t.TargetType {
		case "workspace":
			if isWorkspaceMember {
				return true
			}
		case "member":
			if util.UUIDToString(t.TargetID) == userID {
				return true
			}
		}
	}
	return false
}

func (s *AutomationService) canCreatorInvokeAgent(ctx context.Context, ap db.Automation, agent db.Agent) bool {
	creatorID := util.UUIDToString(ap.CreatedByID)
	if ap.CreatedByType == "member" && util.UUIDToString(agent.OwnerID) == creatorID {
		return true
	}
	if agent.PermissionMode != "public_to" {
		// private (or unknown mode): deny-by-default; only the owner branch
		// above passes. Admins and agent-created automations do not bypass.
		return false
	}
	targets, err := s.Queries.ListAgentInvocationTargets(ctx, agent.ID)
	if err != nil {
		return false
	}
	// Agent-created automations are workspace-internal principals: a workspace
	// target admits them. Member creators must be workspace members.
	workspaceBroad := ap.CreatedByType == "agent"
	isWorkspaceMember := false
	if ap.CreatedByType == "member" {
		if _, err := s.Queries.GetMemberByUserAndWorkspace(ctx, db.GetMemberByUserAndWorkspaceParams{
			UserID:      ap.CreatedByID,
			WorkspaceID: ap.WorkspaceID,
		}); err == nil {
			isWorkspaceMember = true
		}
	}
	for _, t := range targets {
		switch t.TargetType {
		case "workspace":
			if isWorkspaceMember || workspaceBroad {
				return true
			}
		case "member":
			if ap.CreatedByType == "member" && util.UUIDToString(t.TargetID) == creatorID {
				return true
			}
		}
	}
	return false
}
