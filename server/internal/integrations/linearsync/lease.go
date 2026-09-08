package linearsync

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

var errLinearLeaseLost = errors.New("Linear sync lease lost")

type linearLease struct {
	table   string
	id      pgtype.UUID
	owner   string
	attempt int32
}

type linearLeaseKey struct{}

func (w *Worker) withLease(ctx context.Context, table string, id pgtype.UUID, attempt int32) context.Context {
	return context.WithValue(ctx, linearLeaseKey{}, linearLease{table: table, id: id, owner: w.workerID, attempt: attempt})
}

func (w *Worker) startLease(ctx context.Context, table string, id pgtype.UUID, attempt int32) (context.Context, func() error) {
	workCtx, cancelWork := context.WithCancelCause(w.withLease(ctx, table, id, attempt))
	renewCtx, cancelRenew := context.WithCancel(workCtx)
	ticker := time.NewTicker(20 * time.Second)
	done := make(chan struct{})
	go func() {
		defer close(done)
		defer ticker.Stop()
		w.renewLease(renewCtx, ticker.C, cancelWork)
	}()
	return workCtx, func() error {
		cancelRenew()
		<-done
		cause := context.Cause(workCtx)
		cancelWork(context.Canceled)
		return cause
	}
}

// Renewal failure invalidates the work context, including in-flight provider
// requests. A database failure leaves ownership uncertain, so it also stops
// the work; recovery belongs to the next durable claim.
func (w *Worker) renewLease(ctx context.Context, ticks <-chan time.Time, cancel context.CancelCauseFunc) {
	lease := ctx.Value(linearLeaseKey{}).(linearLease)
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticks:
			var rows int64
			var err error
			q := db.New(w.db)
			if lease.table == "linear_sync_inbox" {
				rows, err = q.RenewLinearSyncInbox(ctx, db.RenewLinearSyncInboxParams{ID: lease.id, Secs: linearWorkerLease.Seconds(), LockedBy: pgtype.Text{String: lease.owner, Valid: true}, Attempts: lease.attempt})
			} else {
				rows, err = q.RenewLinearSyncOutbox(ctx, db.RenewLinearSyncOutboxParams{ID: lease.id, Secs: linearWorkerLease.Seconds(), LockedBy: pgtype.Text{String: lease.owner, Valid: true}, Attempts: lease.attempt})
			}
			if ctx.Err() != nil {
				return
			}
			if err != nil || rows != 1 {
				cause := errLinearLeaseLost
				if err != nil {
					cause = fmt.Errorf("%w: renewal: %v", errLinearLeaseLost, err)
				}
				cancel(cause)
				slog.WarnContext(ctx, "linear sync lease lost", "queue", lease.table, "id", uuidToString(lease.id), "attempt", lease.attempt, "error", err)
				return
			}
		}
	}
}

func checkLinearLease(ctx context.Context, executor DBExecutor, lease linearLease, lock bool) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	// The expiry predicate must run after the row lock is acquired. PostgreSQL
	// can evaluate WHERE before waiting for an unchanged tuple's lock, and it
	// does not necessarily re-evaluate that predicate when the lock arrives.
	if lock {
		var lockedID pgtype.UUID
		if err := executor.QueryRow(ctx, `SELECT id FROM `+lease.table+` WHERE id=$1 FOR UPDATE`, lease.id).Scan(&lockedID); err != nil {
			if errors.Is(err, pgx.ErrNoRows) {
				return errLinearLeaseLost
			}
			return err
		}
	}
	query := `SELECT true FROM ` + lease.table + ` WHERE id=$1 AND locked_by=$2 AND attempts=$3 AND locked_until>clock_timestamp() AND processed_at IS NULL AND dead_lettered_at IS NULL`
	var owned bool
	if err := executor.QueryRow(ctx, query, lease.id, lease.owner, lease.attempt).Scan(&owned); err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return errLinearLeaseLost
		}
		return err
	}
	return nil
}

// Check again before each provider mutation. An already accepted remote write
// cannot be rolled back by cancelling HTTP, so the existing stable identities
// remain necessary when the next owner retries.
func (w *Worker) checkLease(ctx context.Context) error {
	if lease, ok := ctx.Value(linearLeaseKey{}).(linearLease); ok {
		return checkLinearLease(ctx, w.db, lease, false)
	}
	return ctx.Err()
}

// Local writes hold the claimed queue row until commit. Reclaimers cannot pass
// that lock, and Commit checks expiry again using PostgreSQL's wall clock.
// The token service is also called directly by HTTP handlers without a queue
// claim; those transactions retain their existing connection-level locking.
func (w *Worker) beginLeaseTx(ctx context.Context) (pgx.Tx, error) {
	tx, err := w.txStarter.Begin(ctx)
	if err != nil {
		return nil, err
	}
	lease, ok := ctx.Value(linearLeaseKey{}).(linearLease)
	if !ok {
		return tx, nil
	}
	if err = checkLinearLease(ctx, tx, lease, true); err != nil {
		_ = tx.Rollback(ctx)
		return nil, err
	}
	return &linearLeaseTx{Tx: tx, lease: lease}, nil
}

// beginLeaseTxAfterLink starts a transaction whose first database lock may be
// a linear_issue_link row. The normal beginLeaseTx deliberately locks the
// claimed inbox/outbox row first, which is correct for most local writes but
// deadlocks with binding removal when both paths touch link and queue rows in
// opposite orders. Callers must perform the non-locking lease check before
// their first local write; Commit/releaseLease performs the authoritative
// queue-row fence after the link mutation.
func (w *Worker) beginLeaseTxAfterLink(ctx context.Context) (pgx.Tx, error) {
	tx, err := w.txStarter.Begin(ctx)
	if err != nil {
		return nil, err
	}
	lease, ok := ctx.Value(linearLeaseKey{}).(linearLease)
	if !ok {
		return tx, nil
	}
	return &linearLeaseTx{Tx: tx, lease: lease}, nil
}

type linearLeaseTx struct {
	pgx.Tx
	lease    linearLease
	released bool
}

func (tx *linearLeaseTx) Commit(ctx context.Context) error {
	if !tx.released {
		if err := checkLinearLease(ctx, tx.Tx, tx.lease, true); err != nil {
			return err
		}
	}
	return tx.Tx.Commit(ctx)
}

func (w *Worker) releaseLease(ctx context.Context, tx pgx.Tx, connectionID pgtype.UUID, processErr error, maxAttempts int32) error {
	leaseTx, ok := tx.(*linearLeaseTx)
	if !ok {
		return errors.New("Linear queue completion requires a claimed transaction")
	}
	lease := leaseTx.lease
	if err := checkLinearLease(ctx, tx, lease, true); err != nil {
		return err
	}
	// Write health before releasing the queue row: the fenced release is the
	// final database operation, so an expiry while waiting for the connection
	// lock rolls the health update back with the rest of the transaction.
	if connectionID.Valid {
		var err error
		if processErr == nil {
			_, err = tx.Exec(ctx, `UPDATE linear_connection SET last_success_at=now(),last_error=NULL,updated_at=now() WHERE id=$1 AND status='active'`, connectionID)
		} else {
			_, err = tx.Exec(ctx, `UPDATE linear_connection SET last_error=$2,updated_at=now() WHERE id=$1 AND status='active'`, connectionID, processErr.Error())
		}
		if err != nil {
			return err
		}
	}
	q := db.New(tx)
	owner := pgtype.Text{String: lease.owner, Valid: true}
	var rows int64
	var err error
	if processErr == nil {
		if lease.table == "linear_sync_inbox" {
			rows, err = q.CompleteLinearSyncInbox(ctx, db.CompleteLinearSyncInboxParams{ID: lease.id, LockedBy: owner, Attempts: lease.attempt})
		} else {
			rows, err = q.CompleteLinearSyncOutbox(ctx, db.CompleteLinearSyncOutboxParams{ID: lease.id, LockedBy: owner, Attempts: lease.attempt})
		}
	} else if lease.attempt >= maxAttempts {
		if lease.table == "linear_sync_inbox" {
			rows, err = q.DeadLetterLinearSyncInbox(ctx, db.DeadLetterLinearSyncInboxParams{ID: lease.id, LockedBy: owner, Attempts: lease.attempt, LastError: pgtype.Text{String: processErr.Error(), Valid: true}})
		} else {
			rows, err = q.DeadLetterLinearSyncOutbox(ctx, db.DeadLetterLinearSyncOutboxParams{ID: lease.id, LockedBy: owner, Attempts: lease.attempt, LastError: pgtype.Text{String: processErr.Error(), Valid: true}})
		}
	} else {
		if lease.table == "linear_sync_inbox" {
			rows, err = q.RetryLinearSyncInbox(ctx, db.RetryLinearSyncInboxParams{ID: lease.id, LockedBy: owner, Attempts: lease.attempt, Secs: retryDelay(lease.attempt).Seconds(), LastError: pgtype.Text{String: processErr.Error(), Valid: true}})
		} else {
			rows, err = q.RetryLinearSyncOutbox(ctx, db.RetryLinearSyncOutboxParams{ID: lease.id, LockedBy: owner, Attempts: lease.attempt, Secs: retryDelay(lease.attempt).Seconds(), LastError: pgtype.Text{String: processErr.Error(), Valid: true}})
		}
	}
	if err != nil {
		return err
	}
	if rows != 1 {
		return errLinearLeaseLost
	}
	leaseTx.released = true
	return nil
}
