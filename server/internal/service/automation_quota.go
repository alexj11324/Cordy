package service

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/entitlement"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// AutomationQuotaMetrics deliberately accepts only bounded labels. Workspace
// identifiers and policy values must never be metric labels.
type AutomationQuotaMetrics interface {
	RecordAutomationQuotaDecision(action, source, result string)
}

// AutomationQuotaExceededError is returned only for an enforce decision whose
// Cloud-provided interval is full. HTTP callers can serialize the facts without
// embedding commercial copy or plan names in OSS.
type AutomationQuotaExceededError struct {
	Used     int64
	Reserved int64
	Limit    int64
	ResetAt  time.Time
}

func (e *AutomationQuotaExceededError) Error() string {
	return "automation run quota exceeded"
}

// AutomationQuotaUsage is the workspace-scoped, policy-neutral API model.
// A disabled/malformed decision returns Enabled=false and leaves all facts nil.
type AutomationQuotaUsage struct {
	Enabled  bool
	Action   string
	Used     *int64
	Reserved *int64
	Total    *int64
	Limit    *int64
	// Reached is nil while observing because observe never rejects a run.
	Reached       *bool
	PeriodStart   *time.Time
	PeriodEnd     *time.Time
	ResetAt       *time.Time
	BlockedCounts map[string]int64
}

type automationQuotaPolicy struct {
	action              entitlement.Action
	limit               int64
	periodStart         time.Time
	periodEnd           time.Time
	resetAt             time.Time
	policyRevision      int64
	subscriptionVersion int64
}

func newAutomationIdempotencyKey() string { return uuid.NewString() }

// NewRequestIdempotencyKey is used only when an HTTP caller omitted its key;
// the generated value scopes idempotency to that single request.
func NewRequestIdempotencyKey() string { return newAutomationIdempotencyKey() }

func validAutomationExecutionSource(source string) bool {
	switch source {
	case "schedule", "manual", "webhook", "api":
		return true
	default:
		return false
	}
}

func (s *AutomationService) quotaPolicy(ctx context.Context, workspaceID pgtype.UUID) (automationQuotaPolicy, bool) {
	if s.Entitlements == nil || !workspaceID.Valid {
		return automationQuotaPolicy{}, false
	}
	decision := s.Entitlements.Gate(ctx, uuid.UUID(workspaceID.Bytes), entitlement.GateAutomationRuns)
	gate := decision.Gate
	if gate.Action == entitlement.ActionOff {
		return automationQuotaPolicy{}, false
	}
	if (gate.Action != entitlement.ActionObserve && gate.Action != entitlement.ActionEnforce) ||
		gate.Limit == nil || *gate.Limit < 0 || gate.PeriodStart == nil || gate.PeriodEnd == nil ||
		gate.ResetAt == nil || !gate.PeriodStart.Before(*gate.PeriodEnd) {
		// A malformed policy is fail-open and, critically, performs no quota-table
		// access. Cloud remains the sole authority over interval construction.
		return automationQuotaPolicy{}, false
	}
	return automationQuotaPolicy{
		action:              gate.Action,
		limit:               int64(*gate.Limit),
		periodStart:         gate.PeriodStart.UTC(),
		periodEnd:           gate.PeriodEnd.UTC(),
		resetAt:             gate.ResetAt.UTC(),
		policyRevision:      decision.PolicyRevision,
		subscriptionVersion: decision.SubscriptionVersion,
	}, true
}

// createAutomationRunWithQuota reserves and links a run in one transaction.
// When policy is off, it intentionally uses the legacy direct INSERT so a
// self-hosted deployment never touches the quota tables.
func (s *AutomationService) createAutomationRunWithQuota(
	ctx context.Context,
	workspaceID pgtype.UUID,
	source, idempotencyKey string,
	params db.CreateAutomationRunParams,
) (db.AutomationRun, bool, error) {
	if !validAutomationExecutionSource(source) {
		return db.AutomationRun{}, false, fmt.Errorf("invalid automation execution source %q", source)
	}
	policy, enabled := s.quotaPolicy(ctx, workspaceID)
	if !enabled {
		run, err := s.Queries.CreateAutomationRun(ctx, params)
		return run, false, err
	}

	tx, err := s.TxStarter.Begin(ctx)
	if err != nil {
		return db.AutomationRun{}, false, fmt.Errorf("begin quota admission: %w", err)
	}
	defer tx.Rollback(ctx)
	qtx := s.Queries.WithTx(tx)
	periodArgs := db.EnsureAutomationQuotaPeriodParams{
		WorkspaceID: workspaceID,
		PeriodStart: pgtype.Timestamptz{Time: policy.periodStart, Valid: true},
		PeriodEnd:   pgtype.Timestamptz{Time: policy.periodEnd, Valid: true},
	}
	period, err := qtx.EnsureAutomationQuotaPeriod(ctx, periodArgs)
	if err != nil {
		return db.AutomationRun{}, false, fmt.Errorf("lock quota period: %w", err)
	}

	existing, err := qtx.GetAutomationQuotaReservationByKey(ctx, db.GetAutomationQuotaReservationByKeyParams{
		WorkspaceID:    workspaceID,
		PeriodStart:    periodArgs.PeriodStart,
		PeriodEnd:      periodArgs.PeriodEnd,
		IdempotencyKey: idempotencyKey,
	})
	if err == nil {
		run, runErr := qtx.GetAutomationRunByQuotaReservation(ctx, existing.ID)
		if errors.Is(runErr, pgx.ErrNoRows) && existing.State == "reserved" {
			// The reservation/run insert normally commits atomically. Recover a
			// manually removed or otherwise orphaned reserved row so the stable
			// idempotency key does not wedge every retry for the whole period.
			if _, releaseErr := settleAutomationQuota(ctx, qtx, existing.ID, false); releaseErr != nil {
				return db.AutomationRun{}, false, fmt.Errorf("release orphaned idempotency reservation: %w", releaseErr)
			}
			period.ReservedCount--
			err = pgx.ErrNoRows // continue through the normal fresh-reservation path
		} else if runErr != nil {
			return db.AutomationRun{}, false, fmt.Errorf("load idempotent quota run: %w", runErr)
		} else {
			if err := tx.Commit(ctx); err != nil {
				return db.AutomationRun{}, false, fmt.Errorf("commit idempotent quota admission: %w", err)
			}
			s.recordAutomationQuotaDecision(policy.action, source, "reused")
			return run, true, nil
		}
	}
	if !errors.Is(err, pgx.ErrNoRows) {
		return db.AutomationRun{}, false, fmt.Errorf("lookup quota reservation: %w", err)
	}

	wouldBlock := period.UsedCount+period.ReservedCount >= policy.limit
	if wouldBlock && policy.action == entitlement.ActionEnforce {
		if _, err := qtx.IncrementAutomationQuotaBlocked(ctx, db.IncrementAutomationQuotaBlockedParams{
			Source: source, WorkspaceID: workspaceID,
			PeriodStart: periodArgs.PeriodStart, PeriodEnd: periodArgs.PeriodEnd,
		}); err != nil {
			return db.AutomationRun{}, false, fmt.Errorf("record blocked quota admission: %w", err)
		}
		if err := tx.Commit(ctx); err != nil {
			return db.AutomationRun{}, false, fmt.Errorf("commit blocked quota admission: %w", err)
		}
		s.recordAutomationQuotaDecision(policy.action, source, "blocked")
		return db.AutomationRun{}, false, &AutomationQuotaExceededError{
			Used: period.UsedCount, Reserved: period.ReservedCount,
			Limit: policy.limit, ResetAt: policy.resetAt,
		}
	}
	// Observe-only would-blocks stay in the bounded decision metric. Durable
	// blocked counts back the usage API only for decisions that reject work.
	reservation, err := qtx.CreateAutomationQuotaReservation(ctx, db.CreateAutomationQuotaReservationParams{
		WorkspaceID: workspaceID, PeriodStart: periodArgs.PeriodStart, PeriodEnd: periodArgs.PeriodEnd,
		PolicyRevision: policy.policyRevision, SubscriptionVersion: policy.subscriptionVersion,
		Source: source, IdempotencyKey: idempotencyKey,
	})
	if err != nil {
		return db.AutomationRun{}, false, fmt.Errorf("create quota reservation: %w", err)
	}
	if _, err := qtx.IncrementAutomationQuotaReserved(ctx, db.IncrementAutomationQuotaReservedParams{
		WorkspaceID: periodArgs.WorkspaceID, PeriodStart: periodArgs.PeriodStart, PeriodEnd: periodArgs.PeriodEnd,
	}); err != nil {
		return db.AutomationRun{}, false, fmt.Errorf("increment reserved quota: %w", err)
	}
	params.QuotaReservationID = reservation.ID
	run, err := qtx.CreateAutomationRun(ctx, params)
	if err != nil {
		return db.AutomationRun{}, false, fmt.Errorf("create quota-linked run: %w", err)
	}
	if err := tx.Commit(ctx); err != nil {
		return db.AutomationRun{}, false, fmt.Errorf("commit quota admission: %w", err)
	}
	result := "admitted"
	if wouldBlock {
		result = "would_block"
	}
	s.recordAutomationQuotaDecision(policy.action, source, result)
	return run, false, nil
}

func (s *AutomationService) recordAutomationQuotaDecision(action entitlement.Action, source, result string) {
	if s.QuotaMetrics != nil {
		s.QuotaMetrics.RecordAutomationQuotaDecision(string(action), source, result)
	}
}

func settleAutomationQuota(ctx context.Context, q *db.Queries, reservationID pgtype.UUID, consume bool) (bool, error) {
	if !reservationID.Valid {
		return false, nil
	}
	var err error
	if consume {
		_, err = q.ConsumeAutomationQuotaReservation(ctx, reservationID)
	} else {
		_, err = q.ReleaseAutomationQuotaReservation(ctx, reservationID)
	}
	if errors.Is(err, pgx.ErrNoRows) {
		return false, nil // terminal replay: reservation already finalized
	}
	return err == nil, err
}

func (s *AutomationService) completeAutomationRun(ctx context.Context, params db.UpdateAutomationRunCompletedParams) (db.AutomationRun, error) {
	row, err := s.Queries.UpdateAutomationRunTerminalWithQuota(ctx, db.UpdateAutomationRunTerminalWithQuotaParams{
		TerminalStatus: "completed",
		Result:         params.Result,
		RunID:          params.ID,
		Consume:        true,
	})
	return automationRunFromTerminalRow(row), err
}

func (s *AutomationService) failAutomationRun(ctx context.Context, params db.UpdateAutomationRunFailedParams) (db.AutomationRun, error) {
	row, err := s.Queries.UpdateAutomationRunTerminalWithQuota(ctx, db.UpdateAutomationRunTerminalWithQuotaParams{
		TerminalStatus: "failed",
		FailureReason:  params.FailureReason,
		ReasonCode:     params.ReasonCode,
		RunID:          params.ID,
	})
	return automationRunFromTerminalRow(row), err
}

func (s *AutomationService) skipAutomationRun(ctx context.Context, params db.UpdateAutomationRunSkippedParams) (db.AutomationRun, error) {
	row, err := s.Queries.UpdateAutomationRunTerminalWithQuota(ctx, db.UpdateAutomationRunTerminalWithQuotaParams{
		TerminalStatus: "skipped",
		FailureReason:  params.FailureReason,
		ReasonCode:     params.ReasonCode,
		RunID:          params.ID,
	})
	return automationRunFromTerminalRow(row), err
}

func automationRunFromTerminalRow(row db.UpdateAutomationRunTerminalWithQuotaRow) db.AutomationRun {
	return db.AutomationRun{
		ID: row.ID, AutomationID: row.AutomationID, TriggerID: row.TriggerID,
		Source: row.Source, Status: row.Status, IssueID: row.IssueID, TaskID: row.TaskID,
		TriggeredAt: row.TriggeredAt, CompletedAt: row.CompletedAt,
		FailureReason: row.FailureReason, TriggerPayload: row.TriggerPayload,
		Result: row.Result, CreatedAt: row.CreatedAt, TeamID: row.TeamID,
		PlannedAt: row.PlannedAt, WebhookDeliveryID: row.WebhookDeliveryID,
		QuotaReservationID: row.QuotaReservationID, ReasonCode: row.ReasonCode,
	}
}

func (s *AutomationService) recoverPartialAutomationRun(ctx context.Context, run db.AutomationRun) (bool, error) {
	rows, err := s.Queries.RecoverPartialAutomationRun(ctx, run.ID)
	return rows > 0, err
}

// FailAutomationRunsByIssue keeps create_issue consumption immutable while
// releasing any still-reserved run_only slots before deletion clears issue_id.
func (s *AutomationService) FailAutomationRunsByIssue(ctx context.Context, issueID pgtype.UUID) error {
	_, err := s.Queries.FailAutomationRunsByIssue(ctx, issueID)
	return err
}

func (s *AutomationService) AutomationQuotaUsage(ctx context.Context, workspaceID pgtype.UUID) (AutomationQuotaUsage, error) {
	policy, enabled := s.quotaPolicy(ctx, workspaceID)
	if !enabled {
		return AutomationQuotaUsage{Enabled: false}, nil
	}
	period, err := s.Queries.GetAutomationQuotaPeriod(ctx, db.GetAutomationQuotaPeriodParams{
		WorkspaceID: workspaceID,
		PeriodStart: pgtype.Timestamptz{Time: policy.periodStart, Valid: true},
		PeriodEnd:   pgtype.Timestamptz{Time: policy.periodEnd, Valid: true},
	})
	if errors.Is(err, pgx.ErrNoRows) {
		period.UsedCount = 0
		period.ReservedCount = 0
	} else if err != nil {
		return AutomationQuotaUsage{}, fmt.Errorf("load automation quota usage: %w", err)
	}
	blockedCounts := make(map[string]int64)
	if len(period.BlockedCounts) > 0 {
		if err := json.Unmarshal(period.BlockedCounts, &blockedCounts); err != nil {
			return AutomationQuotaUsage{}, fmt.Errorf("decode automation quota blocked counts: %w", err)
		}
		if blockedCounts == nil {
			blockedCounts = make(map[string]int64)
		}
	}
	total := period.UsedCount + period.ReservedCount
	var reached *bool
	if policy.action == entitlement.ActionEnforce {
		value := total >= policy.limit
		reached = &value
	}
	return AutomationQuotaUsage{
		Enabled: true, Action: string(policy.action),
		Used: &period.UsedCount, Reserved: &period.ReservedCount, Total: &total,
		Limit: &policy.limit, Reached: reached,
		PeriodStart: &policy.periodStart, PeriodEnd: &policy.periodEnd, ResetAt: &policy.resetAt,
		BlockedCounts: blockedCounts,
	}, nil
}

func (s *AutomationService) QuotaEnabled() bool { return s.Entitlements != nil }

// ReconcileAutomationQuotaReservations repairs crash windows left after a
// reservation/run transaction but before the downstream side effect or normal
// finalizer. The reservation transition remains CAS-based, so replicas may run
// this concurrently without double-adjusting counters.
func (s *AutomationService) ReconcileAutomationQuotaReservations(
	ctx context.Context,
	terminalCreatedBefore time.Time,
	partialCreatedBefore time.Time,
	limit int32,
) (int, error) {
	if !s.QuotaEnabled() || limit <= 0 {
		return 0, nil
	}
	reservations, err := s.Queries.ListRecoverableAutomationQuotaReservations(ctx, db.ListRecoverableAutomationQuotaReservationsParams{
		TerminalCreatedBefore: pgtype.Timestamptz{Time: terminalCreatedBefore.UTC(), Valid: true},
		PartialCreatedBefore:  pgtype.Timestamptz{Time: partialCreatedBefore.UTC(), Valid: true},
		RowLimit:              limit,
	})
	if err != nil {
		return 0, fmt.Errorf("list recoverable quota reservations: %w", err)
	}
	settled := 0
	for _, reservation := range reservations {
		run, runErr := s.Queries.GetAutomationRunByQuotaReservation(ctx, reservation.ID)
		switch {
		case errors.Is(runErr, pgx.ErrNoRows):
			changed, err := settleAutomationQuota(ctx, s.Queries, reservation.ID, false)
			if err != nil {
				return settled, fmt.Errorf("release orphan quota reservation: %w", err)
			}
			if !changed {
				continue
			}
		case runErr != nil:
			return settled, fmt.Errorf("load quota-linked run: %w", runErr)
		case run.Status == "completed":
			changed, err := settleAutomationQuota(ctx, s.Queries, reservation.ID, true)
			if err != nil {
				return settled, fmt.Errorf("consume completed quota reservation: %w", err)
			}
			if !changed {
				continue
			}
		case run.Status == "failed" || run.Status == "skipped":
			changed, err := settleAutomationQuota(ctx, s.Queries, reservation.ID, false)
			if err != nil {
				return settled, fmt.Errorf("release terminal quota reservation: %w", err)
			}
			if !changed {
				continue
			}
		case (run.Source == "manual" || run.Source == "api") &&
			!run.IssueID.Valid && !run.TaskID.Valid &&
			(run.Status == "pending" || run.Status == "issue_created" || run.Status == "running"):
			changed, err := s.recoverPartialAutomationRun(ctx, run)
			if err != nil {
				return settled, fmt.Errorf("recover abandoned quota run: %w", err)
			}
			if !changed {
				continue
			}
		default:
			// Schedule and webhook retries own their partial-state recovery. The
			// query excludes them so this branch is only a defensive guard.
			continue
		}
		settled++
	}
	return settled, nil
}
