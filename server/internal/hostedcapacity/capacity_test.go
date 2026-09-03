package hostedcapacity

import (
	"context"
	"testing"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"

	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

type fakeTx struct {
	pgx.Tx
	committed bool
}

func (t *fakeTx) Commit(context.Context) error   { t.committed = true; return nil }
func (t *fakeTx) Rollback(context.Context) error { return nil }

type fakeTxStarter struct{ tx *fakeTx }

func (f *fakeTxStarter) Begin(context.Context) (pgx.Tx, error) { return f.tx, nil }

// fakeQueries drives Reconcile without a database: the caller seeds the
// ordered installation list and inspects the pause/resume/observation calls.
type fakeQueries struct {
	workspaceMissing bool
	rows             []db.ListActiveChannelInstallationsForCapacityRow

	pausedIDs    [][]pgtype.UUID
	resumedIDs   [][]pgtype.UUID
	observations []db.UpsertRuntimeObservationParams
	listedIDs    []pgtype.UUID
}

func (f *fakeQueries) WithTx(pgx.Tx) Queries { return f }

func (f *fakeQueries) LockWorkspaceForHostedCapacity(_ context.Context, _ pgtype.UUID) (pgtype.UUID, error) {
	if f.workspaceMissing {
		return pgtype.UUID{}, pgx.ErrNoRows
	}
	return pgtype.UUID{Bytes: [16]byte{1}, Valid: true}, nil
}

func (f *fakeQueries) ListActiveChannelInstallationsForCapacity(_ context.Context, workspaceID pgtype.UUID) ([]db.ListActiveChannelInstallationsForCapacityRow, error) {
	f.listedIDs = append(f.listedIDs, workspaceID)
	return f.rows, nil
}

func (f *fakeQueries) PauseChannelInstallationsForHostedCapacity(_ context.Context, ids []pgtype.UUID) (int64, error) {
	f.pausedIDs = append(f.pausedIDs, ids)
	return int64(len(ids)), nil
}

func (f *fakeQueries) ResumeChannelInstallationsForHostedCapacity(_ context.Context, ids []pgtype.UUID) (int64, error) {
	f.resumedIDs = append(f.resumedIDs, ids)
	return int64(len(ids)), nil
}

func (f *fakeQueries) ListHostedInstallationWorkspaces(context.Context) ([]pgtype.UUID, error) {
	return nil, nil
}

func (f *fakeQueries) UpsertRuntimeObservation(_ context.Context, arg db.UpsertRuntimeObservationParams) (db.ChannelInstallationRuntimeObservation, error) {
	f.observations = append(f.observations, arg)
	return db.ChannelInstallationRuntimeObservation{}, nil
}

func pausedAt() pgtype.Timestamptz {
	return pgtype.Timestamptz{Valid: true}
}

func inst(id byte, paused bool) db.ListActiveChannelInstallationsForCapacityRow {
	row := db.ListActiveChannelInstallationsForCapacityRow{
		ID: pgtype.UUID{Bytes: [16]byte{id}, Valid: true},
	}
	if paused {
		row.HostedPausedAt = pausedAt()
	}
	return row
}

func TestReconcile(t *testing.T) {
	ws := pgtype.UUID{Bytes: [16]byte{1}, Valid: true}
	t.Run("keeps the oldest installations and pauses the rest", func(t *testing.T) {
		q := &fakeQueries{rows: []db.ListActiveChannelInstallationsForCapacityRow{
			inst(1, false), inst(2, false), inst(3, false),
		}}
		tx := &fakeTxStarter{tx: &fakeTx{}}
		result, err := Reconcile(context.Background(), q, tx, ws, ptrOf(int64(2)))
		if err != nil {
			t.Fatalf("Reconcile: %v", err)
		}
		if !tx.tx.committed {
			t.Fatal("reconcile must commit")
		}
		if len(result.Resumed) != 0 || len(result.Paused) != 1 || result.Paused[0] != inst(3, false).ID {
			t.Fatalf("result = %+v", result)
		}
		if len(q.pausedIDs) != 1 || len(q.pausedIDs[0]) != 1 {
			t.Fatalf("paused calls = %v", q.pausedIDs)
		}
		if len(q.resumedIDs) != 0 {
			t.Fatalf("resumed calls = %v", q.resumedIDs)
		}
	})
	t.Run("resumes a paused installation that fits again", func(t *testing.T) {
		q := &fakeQueries{rows: []db.ListActiveChannelInstallationsForCapacityRow{
			inst(1, true), inst(2, false),
		}}
		tx := &fakeTxStarter{tx: &fakeTx{}}
		result, err := Reconcile(context.Background(), q, tx, ws, ptrOf(int64(2)))
		if err != nil {
			t.Fatalf("Reconcile: %v", err)
		}
		if len(result.Paused) != 0 || len(result.Resumed) != 1 {
			t.Fatalf("result = %+v", result)
		}
		if len(q.resumedIDs) != 1 || len(q.resumedIDs[0]) != 1 {
			t.Fatalf("resumed calls = %v", q.resumedIDs)
		}
	})
	t.Run("nil limit resumes every paused installation", func(t *testing.T) {
		// Unlimited/bypass: the pause marker is a runtime condition, never a
		// desired state, so nothing stays paused without a cap enforcing it.
		q := &fakeQueries{rows: []db.ListActiveChannelInstallationsForCapacityRow{
			inst(1, true), inst(2, true), inst(3, false),
		}}
		tx := &fakeTxStarter{tx: &fakeTx{}}
		result, err := Reconcile(context.Background(), q, tx, ws, nil)
		if err != nil {
			t.Fatalf("Reconcile: %v", err)
		}
		if len(result.Resumed) != 2 || len(result.Paused) != 0 {
			t.Fatalf("result = %+v", result)
		}
	})
	t.Run("paused installations get an offline observation from the entitlement observer", func(t *testing.T) {
		q := &fakeQueries{rows: []db.ListActiveChannelInstallationsForCapacityRow{
			inst(1, false), inst(2, false),
		}}
		tx := &fakeTxStarter{tx: &fakeTx{}}
		if _, err := Reconcile(context.Background(), q, tx, ws, ptrOf(int64(1))); err != nil {
			t.Fatalf("Reconcile: %v", err)
		}
		if len(q.observations) != 1 {
			t.Fatalf("observations = %d, want 1", len(q.observations))
		}
		got := q.observations[0]
		if got.State != pausedState || got.ErrorCode.String != pausedReason || got.ObserverToken != ObserverToken {
			t.Fatalf("observation = %+v", got)
		}
		if !got.ObservedAt.Valid {
			t.Fatal("observation must carry a timestamp")
		}
	})
	t.Run("already-aligned state writes nothing", func(t *testing.T) {
		// Two fit under the cap and are unpaused; the third is over the cap
		// and already paused — reconcile has nothing to write.
		q := &fakeQueries{rows: []db.ListActiveChannelInstallationsForCapacityRow{
			inst(1, false), inst(2, false), inst(3, true),
		}}
		tx := &fakeTxStarter{tx: &fakeTx{}}
		result, err := Reconcile(context.Background(), q, tx, ws, ptrOf(int64(2)))
		if err != nil {
			t.Fatalf("Reconcile: %v", err)
		}
		if len(result.Paused) != 0 || len(result.Resumed) != 0 {
			t.Fatalf("result = %+v", result)
		}
		if len(q.pausedIDs) != 0 || len(q.resumedIDs) != 0 || len(q.observations) != 0 {
			t.Fatal("an aligned workspace must not be written")
		}
	})
	t.Run("missing workspace is an empty success, not an error", func(t *testing.T) {
		q := &fakeQueries{workspaceMissing: true}
		tx := &fakeTxStarter{tx: &fakeTx{}}
		result, err := Reconcile(context.Background(), q, tx, ws, ptrOf(int64(1)))
		if err != nil {
			t.Fatalf("Reconcile: %v", err)
		}
		if len(result.Paused) != 0 || len(result.Resumed) != 0 {
			t.Fatalf("result = %+v", result)
		}
		if len(q.pausedIDs) != 0 || len(q.resumedIDs) != 0 {
			t.Fatal("a missing workspace must not be written")
		}
	})
}
