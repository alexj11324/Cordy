package hostedcapacity

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"

	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// ObserverToken marks runtime observations written by this package, so the
// supervision surface can tell entitlement-driven pauses apart from
// per-channel connectivity verdicts. Mirrors the Rust observer identity.
const ObserverToken = "managed:entitlement:v1"

const (
	pausedState   = "offline"
	pausedReason  = "hosted_quota_paused"
	pausedSummary = "installation paused: over the workspace's hosted messaging installation capacity"
)

// txStarter abstracts transaction creation; satisfied by *pgxpool.Pool. Kept
// local (the repo's per-package seam pattern) so this package never
// back-references the channel engine.
type txStarter interface {
	Begin(ctx context.Context) (pgx.Tx, error)
}

// Queries is the slice of generated queries reconcile needs. *db.Queries
// satisfies it through the dbQueries adapter; tests supply a fake.
type Queries interface {
	WithTx(tx pgx.Tx) Queries
	LockWorkspaceForHostedCapacity(ctx context.Context, workspaceID pgtype.UUID) (pgtype.UUID, error)
	ListActiveChannelInstallationsForCapacity(ctx context.Context, workspaceID pgtype.UUID) ([]db.ListActiveChannelInstallationsForCapacityRow, error)
	PauseChannelInstallationsForHostedCapacity(ctx context.Context, ids []pgtype.UUID) (int64, error)
	ResumeChannelInstallationsForHostedCapacity(ctx context.Context, ids []pgtype.UUID) (int64, error)
	ListHostedInstallationWorkspaces(ctx context.Context) ([]pgtype.UUID, error)
	UpsertRuntimeObservation(ctx context.Context, arg db.UpsertRuntimeObservationParams) (db.ChannelInstallationRuntimeObservation, error)
}

type dbQueries struct{ *db.Queries }

func (q dbQueries) WithTx(tx pgx.Tx) Queries { return dbQueries{q.Queries.WithTx(tx)} }

// ReconcileResult reports what one reconcile pass changed. Empty slices mean
// the durable pause state already matched the policy.
type ReconcileResult struct {
	Paused  []pgtype.UUID
	Resumed []pgtype.UUID
}

// Reconcile aligns a workspace's durable pause markers with limit. A nil
// limit keeps every installation (bypass/unlimited — resume-all semantics),
// matching the Rust reconcile: the marker is a runtime condition, never a
// desired state, so nothing stays paused without a cap enforcing it.
//
// The whole pass runs in one transaction under the same workspace row lock
// admission takes (LockWorkspaceForHostedCapacity), so a concurrent install
// cannot slip into a slot this pass is about to judge, and a policy change
// cannot interleave with another reconcile. A missing workspace (deleted
// mid-pass) returns an empty result, not an error — there is nothing left to
// reconcile.
func Reconcile(ctx context.Context, q Queries, tx txStarter, workspaceID pgtype.UUID, limit *int64) (ReconcileResult, error) {
	result := ReconcileResult{}
	dbTx, err := tx.Begin(ctx)
	if err != nil {
		return result, fmt.Errorf("begin hosted capacity reconcile: %w", err)
	}
	defer rollbackQuiet(ctx, dbTx)
	qtx := q.WithTx(dbTx)
	if _, err := qtx.LockWorkspaceForHostedCapacity(ctx, workspaceID); err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return result, nil
		}
		return result, fmt.Errorf("lock workspace for hosted capacity: %w", err)
	}
	rows, err := qtx.ListActiveChannelInstallationsForCapacity(ctx, workspaceID)
	if err != nil {
		return result, fmt.Errorf("list installations for hosted capacity: %w", err)
	}
	keep := len(rows)
	if limit != nil && int(*limit) < keep {
		keep = int(*limit)
	}
	now := pgtype.Timestamptz{Time: time.Now().UTC(), Valid: true}
	for index, row := range rows {
		paused := row.HostedPausedAt.Valid
		if index < keep {
			if paused {
				result.Resumed = append(result.Resumed, row.ID)
			}
			continue
		}
		if !paused {
			result.Paused = append(result.Paused, row.ID)
		}
	}
	if len(result.Paused) > 0 {
		if _, err := qtx.PauseChannelInstallationsForHostedCapacity(ctx, result.Paused); err != nil {
			return result, fmt.Errorf("pause installations for hosted capacity: %w", err)
		}
		for _, id := range result.Paused {
			if _, err := qtx.UpsertRuntimeObservation(ctx, db.UpsertRuntimeObservationParams{
				InstallationID: id,
				State:          pausedState,
				ObservedAt:     now,
				ErrorCode:      text(pausedReason),
				ErrorSummary:   text(pausedSummary),
				ObserverToken:  ObserverToken,
			}); err != nil {
				return result, fmt.Errorf("record hosted pause observation: %w", err)
			}
		}
	}
	if len(result.Resumed) > 0 {
		if _, err := qtx.ResumeChannelInstallationsForHostedCapacity(ctx, result.Resumed); err != nil {
			return result, fmt.Errorf("resume installations for hosted capacity: %w", err)
		}
	}
	if err := dbTx.Commit(ctx); err != nil {
		return ReconcileResult{}, fmt.Errorf("commit hosted capacity reconcile: %w", err)
	}
	return result, nil
}

func rollbackQuiet(ctx context.Context, tx pgx.Tx) {
	_ = tx.Rollback(ctx)
}

func text(value string) pgtype.Text {
	return pgtype.Text{String: value, Valid: true}
}

// Limiter is the handler-facing service: resolve the policy, reconcile the
// durable markers, and hand the install path its limit. The reconcile-on-
// resolve mirrors the Rust flow, so an upgrade or downgrade is applied the
// moment any install asks, not only on the worker's next sweep.
type Limiter struct {
	resolver *Resolver
	queries  Queries
	tx       txStarter
	logger   *slog.Logger
}

// NewLimiter binds the limiter. A nil resolver yields a limiter whose every
// call is a no-op nil limit (the disabled deployment), so handlers never
// branch on enablement themselves.
func NewLimiter(resolver *Resolver, q *db.Queries, tx txStarter, logger *slog.Logger) *Limiter {
	if logger == nil {
		logger = slog.Default()
	}
	return &Limiter{resolver: resolver, queries: dbQueries{q}, tx: tx, logger: logger}
}

// InstallationLimit resolves the workspace's hosted installation cap and
// reconciles the pause markers to match. A nil limit means no admission
// check. Unavailable fails closed with ErrQuotaUnavailable; a reconcile
// failure also fails closed — handing out a limit the host could not enforce
// would let a workspace grow past its cap.
func (l *Limiter) InstallationLimit(ctx context.Context, workspaceID pgtype.UUID) (*int64, error) {
	if l == nil || l.resolver == nil || !l.resolver.Enabled() {
		return nil, nil
	}
	policy := l.resolver.Resolve(ctx, workspaceID)
	if policy.Unavailable() {
		return nil, ErrQuotaUnavailable
	}
	if _, err := Reconcile(ctx, l.queries, l.tx, workspaceID, policy.Limit()); err != nil {
		l.logger.WarnContext(ctx, "failed to reconcile hosted installation capacity",
			"workspace_id", workspaceUUIDString(workspaceID), "error", err.Error())
		return nil, ErrQuotaUnavailable
	}
	return policy.Limit(), nil
}

func workspaceUUIDString(id pgtype.UUID) string {
	if !id.Valid {
		return ""
	}
	return uuid.UUID(id.Bytes).String()
}
