package handler

import (
	"context"
	"errors"
	"github.com/jackc/pgx/v5/pgconn"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgtype"
	linearapi "github.com/orvilo-ai/orvilo/server/internal/integrations/linear"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

func TestLinearWorkerRejectsExpiredOutboxCompletion(t *testing.T) {
	f := setupLinearWorker(t, "publish", &fakeLinearAPI{})
	issueID := dbfx.Issue(t, "Expired lease", testutil.Cols{"project_id": f.projectID})
	claim, ok, err := f.worker.claimOutbox(context.Background())
	if err != nil || !ok {
		t.Fatalf("claim: %v, %v", ok, err)
	}
	expireLinearLease(t, "linear_sync_outbox", claim.ID)
	if err = f.worker.handleOutbox(context.Background(), claim); err == nil {
		t.Error("expired worker successfully published and acknowledged its outbox")
	}
	var links int
	if err = testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_issue_link WHERE orvilo_issue_id=$1`, issueID).Scan(&links); err != nil {
		t.Fatal(err)
	}
	if links != 0 {
		t.Errorf("expired worker committed %d links", links)
	}
	var processed bool
	if err = testPool.QueryRow(context.Background(), `SELECT processed_at IS NOT NULL FROM linear_sync_outbox WHERE id=$1`, claim.ID).Scan(&processed); err != nil {
		t.Fatal(err)
	}
	if processed {
		t.Error("expired worker completed the queue")
	}
}

func TestLinearWorkerOldAttemptCannotFinishOrSetHealth(t *testing.T) {
	for _, processErr := range []error{nil, errors.New("stale failure")} {
		name := "success"
		if processErr != nil {
			name = "failure"
		}
		t.Run(name, func(t *testing.T) {
			f := setupLinearWorker(t, "publish", &fakeLinearAPI{})
			dbfx.Issue(t, "New attempt", testutil.Cols{"project_id": f.projectID})
			old, ok, err := f.worker.claimOutbox(context.Background())
			if err != nil || !ok {
				t.Fatalf("claim: %v, %v", ok, err)
			}
			expireLinearLease(t, "linear_sync_outbox", old.ID)
			current, ok, err := f.worker.claimOutbox(context.Background())
			if err != nil || !ok {
				t.Fatalf("reclaim: %v, %v", ok, err)
			}
			if _, err = testPool.Exec(context.Background(), `UPDATE linear_connection SET last_error='current owner health' WHERE id=$1`, f.connectionID); err != nil {
				t.Fatal(err)
			}
			f.worker.finish(context.Background(), "linear_sync_outbox", old.ID, parseUUID(f.connectionID), old.Attempts, old.MaxAttempts, processErr)
			var owner pgtype.Text
			var processed bool
			var attempt int32
			if err = testPool.QueryRow(context.Background(), `SELECT locked_by,processed_at IS NOT NULL,attempts FROM linear_sync_outbox WHERE id=$1`, old.ID).Scan(&owner, &processed, &attempt); err != nil {
				t.Fatal(err)
			}
			if !owner.Valid || owner.String != f.worker.workerID || processed || attempt != current.Attempts {
				t.Errorf("stale attempt changed current claim: owner=%v processed=%v attempt=%d", owner, processed, attempt)
			}
			var health pgtype.Text
			if err = testPool.QueryRow(context.Background(), `SELECT last_error FROM linear_connection WHERE id=$1`, f.connectionID).Scan(&health); err != nil {
				t.Fatal(err)
			}
			if !health.Valid || health.String != "current owner health" {
				t.Errorf("stale attempt changed health: %v", health)
			}
		})
	}
}

type pausedLinearListAPI struct {
	*fakeLinearAPI
	entered chan struct{}
	resume  chan struct{}
}

func (a *pausedLinearListAPI) ListIssues(ctx context.Context, token, projectID, teamID string) ([]linearapi.Issue, error) {
	close(a.entered)
	select {
	case <-a.resume:
	case <-ctx.Done():
		return nil, ctx.Err()
	}
	return a.fakeLinearAPI.ListIssues(ctx, token, projectID, teamID)
}

func TestLinearWorkerLosingInboxLeaseCannotApplyRemote(t *testing.T) {
	api := &fakeLinearAPI{listed: []linearapi.Issue{{ID: "70000000-0000-0000-0000-000000000001", Identifier: "ENG-7", Title: "Remote after loss", ProjectID: "linear-project", TeamID: "linear-team", UpdatedAt: time.Now()}}}
	f := setupLinearWorker(t, "import", api)
	paused := &pausedLinearListAPI{fakeLinearAPI: api, entered: make(chan struct{}), resume: make(chan struct{})}
	f.worker.api = paused
	dbfx.Insert(t, "linear_sync_inbox", testutil.Cols{"connection_id": f.connectionID, "delivery_id": "lease-loss-import", "event_type": "binding_poll", "payload": map[string]any{"binding_id": f.bindingID}})
	claim, ok, err := f.worker.claimInbox(context.Background())
	if err != nil || !ok {
		t.Fatalf("claim: %v, %v", ok, err)
	}
	done := make(chan error, 1)
	go func() { done <- f.worker.handleInbox(context.Background(), claim) }()
	<-paused.entered
	expireLinearLease(t, "linear_sync_inbox", claim.ID)
	next := NewLinearWorker(testPool, testPool, f.box, api, "client", "secret", true, true)
	if _, ok, err = next.claimInbox(context.Background()); err != nil || !ok {
		close(paused.resume)
		<-done
		t.Fatalf("reclaim: %v, %v", ok, err)
	}
	close(paused.resume)
	if err = <-done; err == nil {
		t.Error("lost inbox lease still applied remote data")
	}
	var count int
	if err = testPool.QueryRow(context.Background(), `SELECT count(*) FROM issue WHERE project_id=$1 AND origin_type='linear'`, f.projectID).Scan(&count); err != nil {
		t.Fatal(err)
	}
	if count != 0 {
		t.Errorf("lost inbox lease committed %d imported issues", count)
	}
}

func expireLinearLease(t *testing.T, table string, id pgtype.UUID) {
	t.Helper()
	if _, err := testPool.Exec(context.Background(), `UPDATE `+table+` SET locked_until=clock_timestamp()-interval '1 second' WHERE id=$1`, id); err != nil {
		t.Fatal(err)
	}
}

// The request blocks on its real context. Manual renewal ticks make both
// ownership loss and database uncertainty deterministic without short timers.
type cancelledLinearCreateAPI struct {
	*fakeLinearAPI
	entered chan struct{}
}

func (a *cancelledLinearCreateAPI) CreateIssue(ctx context.Context, _ string, _ linearapi.IssueInput) (linearapi.Issue, error) {
	close(a.entered)
	<-ctx.Done()
	return linearapi.Issue{}, ctx.Err()
}

type renewalFailureDB struct{ dbExecutor }

func (d renewalFailureDB) Exec(ctx context.Context, query string, args ...any) (pgconn.CommandTag, error) {
	if strings.Contains(query, "-- name: RenewLinearSync") {
		return pgconn.CommandTag{}, errors.New("renewal database unavailable")
	}
	return d.dbExecutor.Exec(ctx, query, args...)
}

func TestLinearWorkerLeaseLossCancelsProvider(t *testing.T) {
	for _, failure := range []string{"owner_changed", "database_error", "expired"} {
		t.Run(failure, func(t *testing.T) {
			f := setupLinearWorker(t, "publish", &fakeLinearAPI{})
			dbfx.Issue(t, "Cancelled provider request", testutil.Cols{"project_id": f.projectID})
			claim, ok, err := f.worker.claimOutbox(context.Background())
			if err != nil || !ok {
				t.Fatalf("claim: %v %v", ok, err)
			}
			api := &cancelledLinearCreateAPI{fakeLinearAPI: f.api, entered: make(chan struct{})}
			f.worker.api = api
			if failure == "database_error" {
				f.worker.db = renewalFailureDB{f.worker.db}
			}
			ctx, cancel := context.WithCancelCause(f.worker.withLease(context.Background(), "linear_sync_outbox", claim.ID, claim.Attempts))
			defer cancel(context.Canceled)
			handled := make(chan error, 1)
			go func() { handled <- f.worker.handleOutbox(ctx, claim) }()
			select {
			case <-api.entered:
			case err := <-handled:
				t.Fatalf("provider not called: %v", err)
			case <-time.After(5 * time.Second):
				t.Fatal("provider did not start")
			}
			if failure != "database_error" {
				expireLinearLease(t, "linear_sync_outbox", claim.ID)
			}
			if failure == "owner_changed" {
				next := NewLinearWorker(testPool, testPool, f.box, f.api, "client", "secret", true, true)
				if _, ok, err = next.claimOutbox(context.Background()); err != nil || !ok {
					t.Fatalf("reclaim: %v %v", ok, err)
				}
			}
			ticks := make(chan time.Time, 1)
			ticks <- time.Now()
			renewed := make(chan struct{})
			go func() { defer close(renewed); f.worker.renewLease(ctx, ticks, cancel) }()
			select {
			case err = <-handled:
				if !errors.Is(err, context.Canceled) {
					t.Errorf("provider result: %v", err)
				}
			case <-time.After(5 * time.Second):
				cancel(context.Canceled)
				<-handled
				t.Fatal("renewal failure did not cancel provider")
			}
			<-renewed
			if !errors.Is(context.Cause(ctx), errLinearLeaseLost) {
				t.Errorf("cause=%v", context.Cause(ctx))
			}
		})
	}
}

type acceptedLinearCreateAPI struct {
	*fakeLinearAPI
	entered chan struct{}
	resume  chan struct{}
}

func (a *acceptedLinearCreateAPI) CreateIssue(ctx context.Context, token string, input linearapi.IssueInput) (linearapi.Issue, error) {
	input.Description = linearapi.DescriptionWithOrviloMarker(input.Description, input.OrviloIssueID)
	remote, err := a.fakeLinearAPI.CreateIssue(ctx, token, input)
	close(a.entered)
	<-a.resume
	return remote, err
}

func TestLinearWorkerRecoversRemoteCreateAfterLeaseLoss(t *testing.T) {
	f := setupLinearWorker(t, "publish", &fakeLinearAPI{})
	issueID := dbfx.Issue(t, "Remote committed before loss", testutil.Cols{"project_id": f.projectID})
	claim, ok, err := f.worker.claimOutbox(context.Background())
	if err != nil || !ok {
		t.Fatalf("claim: %v %v", ok, err)
	}
	api := &acceptedLinearCreateAPI{fakeLinearAPI: f.api, entered: make(chan struct{}), resume: make(chan struct{})}
	f.worker.api = api
	handled := make(chan error, 1)
	go func() { handled <- f.worker.handleOutbox(context.Background(), claim) }()
	<-api.entered
	expireLinearLease(t, "linear_sync_outbox", claim.ID)
	next := NewLinearWorker(testPool, testPool, f.box, f.api, "client", "secret", true, true)
	retaken, ok, err := next.claimOutbox(context.Background())
	if err != nil || !ok {
		close(api.resume)
		<-handled
		t.Fatalf("reclaim: %v %v", ok, err)
	}
	close(api.resume)
	if err = <-handled; !errors.Is(err, errLinearLeaseLost) {
		t.Fatalf("old owner result=%v", err)
	}
	var links int
	if err = testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_issue_link WHERE orvilo_issue_id=$1`, issueID).Scan(&links); err != nil {
		t.Fatal(err)
	}
	if links != 0 {
		t.Fatalf("old owner committed %d links", links)
	}
	if err = next.handleOutbox(context.Background(), retaken); err != nil {
		t.Fatal(err)
	}
	creates, updates, _, _ := f.api.calls()
	if creates != 1 || updates != 1 {
		t.Errorf("create/update=%d/%d, want 1/1", creates, updates)
	}
	var processed bool
	if err = testPool.QueryRow(context.Background(), `SELECT processed_at IS NOT NULL FROM linear_sync_outbox WHERE id=$1`, claim.ID).Scan(&processed); err != nil {
		t.Fatal(err)
	}
	if !processed {
		t.Error("new owner did not complete recovered outbox")
	}
}

func TestLinearWorkerLostLeaseCannotImportComment(t *testing.T) {
	f := setupLinearWorker(t, "two_way", &fakeLinearAPI{})
	issueID := dbfx.Issue(t, "Comment host", testutil.Cols{"project_id": f.projectID})
	if !f.worker.processOneOutbox(context.Background()) {
		t.Fatal("issue did not publish")
	}
	id := dbfx.Insert(t, "linear_sync_inbox", testutil.Cols{"connection_id": f.connectionID, "delivery_id": "lease-comment", "event_type": "Comment", "payload": map[string]any{}})
	claim, ok, err := f.worker.claimInbox(context.Background())
	if err != nil || !ok {
		t.Fatalf("claim: %v %v", ok, err)
	}
	expireLinearLease(t, "linear_sync_inbox", parseUUID(id))
	b, err := f.worker.loadBinding(context.Background(), parseUUID(f.bindingID))
	if err != nil {
		t.Fatal(err)
	}
	remote := linearapi.Comment{ID: "remote-late-comment", Body: "Late", UpdatedAt: time.Now()}
	remote.Issue.ID = linkedRemoteID(t, issueID)
	ctx := f.worker.withLease(context.Background(), "linear_sync_inbox", claim.ID, claim.Attempts)
	if err = f.worker.applyLinearComment(ctx, b, remote, false); !errors.Is(err, errLinearLeaseLost) {
		t.Errorf("comment result=%v", err)
	}
	var count int
	if err = testPool.QueryRow(context.Background(), `SELECT count(*) FROM comment WHERE issue_id=$1`, issueID).Scan(&count); err != nil {
		t.Fatal(err)
	}
	if count != 0 {
		t.Errorf("expired owner imported %d comments", count)
	}
}

func TestLinearWorkerRechecksExpiryAtLocalCommit(t *testing.T) {
	f := setupLinearWorker(t, "publish", &fakeLinearAPI{})
	dbfx.Issue(t, "Transaction expires", testutil.Cols{"project_id": f.projectID})
	claim, ok, err := f.worker.claimOutbox(context.Background())
	if err != nil || !ok {
		t.Fatalf("claim: %v %v", ok, err)
	}
	ctx := f.worker.withLease(context.Background(), "linear_sync_outbox", claim.ID, claim.Attempts)
	tx, err := f.worker.beginLeaseTx(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer tx.Rollback(context.Background())
	if _, err = tx.Exec(ctx, `UPDATE linear_connection SET last_error='must roll back' WHERE id=$1`, f.connectionID); err != nil {
		t.Fatal(err)
	}
	// Advance this transaction's lease state directly; no elapsed-time race.
	if _, err = tx.Exec(ctx, `UPDATE linear_sync_outbox SET locked_until=clock_timestamp()-interval '1 second' WHERE id=$1`, claim.ID); err != nil {
		t.Fatal(err)
	}
	if err = tx.Commit(ctx); !errors.Is(err, errLinearLeaseLost) {
		t.Fatalf("commit result=%v", err)
	}
	if err = tx.Rollback(context.Background()); err != nil {
		t.Fatal(err)
	}
	var health pgtype.Text
	if err = testPool.QueryRow(ctx, `SELECT last_error FROM linear_connection WHERE id=$1`, f.connectionID).Scan(&health); err != nil {
		t.Fatal(err)
	}
	if health.Valid {
		t.Errorf("expired transaction committed health: %v", health)
	}
}

// PostgreSQL can evaluate a WHERE predicate before waiting for an unchanged
// tuple's row lock. These tests observe that actual wait, let the database
// clock reach the lease deadline, then release the blocker without updating
// the tuple. A pre-lock expiry check would now incorrectly authorize work.
func TestLinearWorkerLeaseExpiryWhileWaitingForRowLock(t *testing.T) {
	for _, table := range []string{"linear_sync_inbox", "linear_sync_outbox"} {
		for _, operation := range []string{"renew", "begin_local_transaction"} {
			t.Run(table+"/"+operation, func(t *testing.T) {
				f := setupLinearWorker(t, "publish", &fakeLinearAPI{})
				ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
				defer cancel()
				var id pgtype.UUID
				var attempt int32
				if table == "linear_sync_inbox" {
					dbfx.Insert(t, table, testutil.Cols{"connection_id": f.connectionID, "delivery_id": "lock-wait-expiry", "event_type": "binding_poll", "payload": map[string]any{"binding_id": f.bindingID}})
					claim, ok, err := f.worker.claimInbox(ctx)
					if err != nil || !ok {
						t.Fatalf("claim: %v %v", ok, err)
					}
					id, attempt = claim.ID, claim.Attempts
				} else {
					dbfx.Issue(t, "Lock wait expiry", testutil.Cols{"project_id": f.projectID})
					claim, ok, err := f.worker.claimOutbox(ctx)
					if err != nil || !ok {
						t.Fatalf("claim: %v %v", ok, err)
					}
					id, attempt = claim.ID, claim.Attempts
				}
				// Two seconds leaves ample time to observe the lock wait. The
				// release condition below is the database clock, not a sleep.
				if _, err := testPool.Exec(ctx, `UPDATE `+table+` SET locked_until=clock_timestamp()+interval '2 seconds' WHERE id=$1`, id); err != nil {
					t.Fatal(err)
				}
				blocker, err := testPool.Begin(ctx)
				if err != nil {
					t.Fatal(err)
				}
				defer blocker.Rollback(context.Background())
				var lockedID pgtype.UUID
				if err = blocker.QueryRow(ctx, `SELECT id FROM `+table+` WHERE id=$1 FOR UPDATE`, id).Scan(&lockedID); err != nil {
					t.Fatal(err)
				}
				waiting, err := testPool.Acquire(ctx)
				if err != nil {
					t.Fatal(err)
				}
				defer waiting.Release()
				result := make(chan linearLockWaitResult, 1)
				go func() {
					if operation == "renew" {
						q := db.New(waiting)
						var rows int64
						var err error
						owner := pgtype.Text{String: f.worker.workerID, Valid: true}
						if table == "linear_sync_inbox" {
							rows, err = q.RenewLinearSyncInbox(ctx, db.RenewLinearSyncInboxParams{ID: id, LockedBy: owner, Attempts: attempt, Secs: linearWorkerLease.Seconds()})
						} else {
							rows, err = q.RenewLinearSyncOutbox(ctx, db.RenewLinearSyncOutboxParams{ID: id, LockedBy: owner, Attempts: attempt, Secs: linearWorkerLease.Seconds()})
						}
						result <- linearLockWaitResult{rows: rows, err: err}
						return
					}
					worker := *f.worker
					worker.txStarter = waiting
					tx, err := worker.beginLeaseTx(worker.withLease(ctx, table, id, attempt))
					if err == nil {
						_ = tx.Rollback(ctx)
					}
					result <- linearLockWaitResult{err: err}
				}()
				waitLinearLeaseLockAndExpiry(t, ctx, table, id, waiting.Conn().PgConn().PID(), blocker.Conn().PgConn().PID())
				if err = blocker.Commit(ctx); err != nil {
					t.Fatal(err)
				}
				select {
				case got := <-result:
					if operation == "renew" {
						if got.err != nil {
							t.Fatal(got.err)
						}
						if got.rows != 0 {
							t.Errorf("renewal revived an expired lease after row-lock wait: rows=%d", got.rows)
						}
					} else if !errors.Is(got.err, errLinearLeaseLost) {
						t.Errorf("local transaction accepted expired lease after row-lock wait: %v", got.err)
					}
				case <-ctx.Done():
					t.Fatal(ctx.Err())
				}
			})
		}
	}
}

type linearLockWaitResult struct {
	rows int64
	err  error
}

func waitLinearLeaseLockAndExpiry(t *testing.T, ctx context.Context, table string, id pgtype.UUID, waitingPID, blockingPID uint32) {
	t.Helper()
	observedWait := false
	for {
		var blocked, expired bool
		err := testPool.QueryRow(ctx, `SELECT $2=ANY(pg_blocking_pids($1)),clock_timestamp()>=locked_until FROM `+table+` WHERE id=$3`, waitingPID, blockingPID, id).Scan(&blocked, &expired)
		if err != nil {
			t.Fatal(err)
		}
		if !observedWait {
			if expired {
				t.Fatal("lease expired before PostgreSQL lock wait was observed")
			}
			if blocked {
				observedWait = true
			}
		} else {
			if !blocked {
				t.Fatal("query stopped waiting before blocker was released")
			}
			if expired {
				return
			}
		}
	}
}

func TestLinearWorkerRenewsOwnedUnexpiredLease(t *testing.T) {
	for _, table := range []string{"linear_sync_inbox", "linear_sync_outbox"} {
		t.Run(table, func(t *testing.T) {
			f := setupLinearWorker(t, "publish", &fakeLinearAPI{})
			ctx := context.Background()
			var id pgtype.UUID
			var attempt int32
			if table == "linear_sync_inbox" {
				dbfx.Insert(t, table, testutil.Cols{"connection_id": f.connectionID, "delivery_id": "renew-live", "event_type": "binding_poll", "payload": map[string]any{"binding_id": f.bindingID}})
				claim, ok, err := f.worker.claimInbox(ctx)
				if err != nil || !ok {
					t.Fatalf("claim: %v %v", ok, err)
				}
				id, attempt = claim.ID, claim.Attempts
			} else {
				dbfx.Issue(t, "Live renewal", testutil.Cols{"project_id": f.projectID})
				claim, ok, err := f.worker.claimOutbox(ctx)
				if err != nil || !ok {
					t.Fatalf("claim: %v %v", ok, err)
				}
				id, attempt = claim.ID, claim.Attempts
			}
			var before time.Time
			if err := testPool.QueryRow(ctx, `UPDATE `+table+` SET locked_until=clock_timestamp()+interval '5 seconds' WHERE id=$1 RETURNING locked_until`, id).Scan(&before); err != nil {
				t.Fatal(err)
			}
			owner := pgtype.Text{String: f.worker.workerID, Valid: true}
			q := db.New(testPool)
			var rows int64
			var err error
			if table == "linear_sync_inbox" {
				rows, err = q.RenewLinearSyncInbox(ctx, db.RenewLinearSyncInboxParams{ID: id, LockedBy: owner, Attempts: attempt, Secs: linearWorkerLease.Seconds()})
			} else {
				rows, err = q.RenewLinearSyncOutbox(ctx, db.RenewLinearSyncOutboxParams{ID: id, LockedBy: owner, Attempts: attempt, Secs: linearWorkerLease.Seconds()})
			}
			if err != nil || rows != 1 {
				t.Fatalf("renew: rows=%d err=%v", rows, err)
			}
			var renewed bool
			if err = testPool.QueryRow(ctx, `SELECT locked_until>$2 AND locked_by=$3 AND attempts=$4 AND processed_at IS NULL FROM `+table+` WHERE id=$1`, id, before, owner, attempt).Scan(&renewed); err != nil {
				t.Fatal(err)
			}
			if !renewed {
				t.Error("renewal did not extend the same live claim")
			}
		})
	}
}
