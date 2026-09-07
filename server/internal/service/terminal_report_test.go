package service

import (
	"context"
	"errors"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/orvilo-ai/orvilo/server/internal/events"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
	"github.com/orvilo-ai/orvilo/server/internal/util"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
	"github.com/orvilo-ai/orvilo/server/pkg/dbid"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

type reportFixture struct {
	svc      *TaskService
	fx       *testutil.Fixture
	task     db.AgentTaskQueue
	identity protocol.TerminalReportIdentity
}

func newReportFixture(t *testing.T, retry bool) reportFixture {
	t.Helper()
	pool := newResolveOriginatorPool(t)
	workspaceID, userID, agentID, issueID := seedAttributionFixture(t, pool)
	fx := testutil.New(pool, workspaceID, userID)
	var runtimeID string
	fx.QueryRow(t, `SELECT runtime_id::text FROM agent WHERE id=$1`, agentID).Scan(&runtimeID)
	maxAttempts := 1
	if retry {
		maxAttempts = 2
	}
	taskID := fx.Task(t, agentID, testutil.Cols{"runtime_id": runtimeID, "issue_id": issueID, "status": "running", "dispatched_at": time.Now().Add(-time.Second), "started_at": time.Now(), "attempt": 0, "max_attempts": maxAttempts})
	fx.Cleanup(t, `DELETE FROM comment WHERE source_task_id=$1`, taskID)
	fx.Cleanup(t, `DELETE FROM agent_task_queue WHERE parent_task_id=$1`, taskID)
	fx.Cleanup(t, `DELETE FROM terminal_report_receipt WHERE task_id=$1`, taskID)
	svc := &TaskService{Queries: db.New(pool), TxStarter: pool, Bus: events.New()}
	task, err := svc.Queries.GetAgentTask(context.Background(), util.MustParseUUID(taskID))
	if err != nil {
		t.Fatal(err)
	}
	return reportFixture{svc: svc, fx: fx, task: task, identity: protocol.TerminalReportIdentity{ReportID: util.UUIDToString(dbid.NewV7()), ClaimFence: strconv.FormatInt(TaskClaimFence(task), 10), PayloadSHA256: strings.Repeat("a", 64)}}
}

func mustTerminalReport(t *testing.T, identity protocol.TerminalReportIdentity) *TerminalReport {
	t.Helper()
	report, err := NewTerminalReport(identity)
	if err != nil {
		t.Fatal(err)
	}
	return report
}

func (f reportFixture) send(ctx context.Context, kind string, report *TerminalReport, hook TerminalTaskTxHook) (*db.AgentTaskQueue, error) {
	if kind == "complete" {
		return f.svc.CompleteTaskWithTerminalReport(ctx, f.task.ID, []byte(`{"output":"Implemented the terminal report result."}`), "", "", "", false, "", "", hook, report)
	}
	reason := "agent_error.process_failure"
	if kind == "retry" {
		reason = "timeout"
	}
	return f.svc.FailTaskWithTerminalReport(ctx, f.task.ID, "worker failed", "", "", "", reason, false, "", "", hook, report)
}

func TestTerminalReportConcurrentReplayIsOneCommittedOutcome(t *testing.T) {
	for _, kind := range []string{"complete", "fail", "retry"} {
		t.Run(kind, func(t *testing.T) {
			f := newReportFixture(t, kind == "retry")
			reports := []*TerminalReport{mustTerminalReport(t, f.identity), mustTerminalReport(t, f.identity)}
			var hooks atomic.Int32
			hook := func(context.Context, pgx.Tx, *db.Queries, db.AgentTaskQueue) error { hooks.Add(1); return nil }
			start := make(chan struct{})
			errs := make([]error, 2)
			var wg sync.WaitGroup
			for i := range reports {
				wg.Add(1)
				go func(i int) {
					defer wg.Done()
					<-start
					_, errs[i] = f.send(context.Background(), kind, reports[i], hook)
				}(i)
			}
			close(start)
			wg.Wait()
			for _, err := range errs {
				if err != nil {
					t.Fatal(err)
				}
			}
			if hooks.Load() != 1 {
				t.Fatalf("terminal hook ran %d times", hooks.Load())
			}
			if reports[0].Ack == nil || reports[1].Ack == nil || *reports[0].Ack != *reports[1].Ack {
				t.Fatalf("replay acknowledgements differ: %+v %+v", reports[0].Ack, reports[1].Ack)
			}
			if reports[0].Replayed == reports[1].Replayed {
				t.Fatal("expected exactly one first acceptance and one replay")
			}
			wantComments, wantRetries := 1, 0
			if kind == "retry" {
				wantComments, wantRetries = 0, 1
			}
			if got := f.fx.Count(t, `SELECT count(*) FROM comment WHERE source_task_id=$1`, f.task.ID); got != wantComments {
				t.Fatalf("outcome comments=%d want %d", got, wantComments)
			}
			if got := f.fx.Count(t, `SELECT count(*) FROM agent_task_queue WHERE parent_task_id=$1`, f.task.ID); got != wantRetries {
				t.Fatalf("retry children=%d want %d", got, wantRetries)
			}
			if got := f.fx.Count(t, `SELECT count(*) FROM terminal_report_receipt WHERE task_id=$1`, f.task.ID); got != 1 {
				t.Fatalf("receipts=%d want 1", got)
			}
		})
	}
}

func TestTerminalReportReceiptDeletedWithTaskBatch(t *testing.T) {
	f := newReportFixture(t, false)
	ctx := context.Background()
	if _, err := f.send(ctx, "complete", mustTerminalReport(t, f.identity), nil); err != nil {
		t.Fatal(err)
	}
	if got := f.fx.Count(t, `SELECT count(*) FROM terminal_report_receipt WHERE task_id=$1`, f.task.ID); got != 1 {
		t.Fatalf("receipt count before deletion = %d", got)
	}
	if err := f.svc.Queries.DeleteTaskBatch(ctx, []pgtype.UUID{f.task.ID}); err != nil {
		t.Fatal(err)
	}
	if got := f.fx.Count(t, `SELECT count(*) FROM terminal_report_receipt WHERE task_id=$1`, f.task.ID); got != 0 {
		t.Fatalf("task deletion orphaned %d receipts", got)
	}
}

func TestTerminalReportRollsBackWithoutWaitingOnIssueWriter(t *testing.T) {
	for _, kind := range []string{"complete", "fail"} {
		t.Run(kind, func(t *testing.T) {
			f := newReportFixture(t, false)
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()
			writer, err := f.fx.Pool.Begin(ctx)
			if err != nil {
				t.Fatal(err)
			}
			defer writer.Rollback(context.Background())
			if _, err := writer.Exec(ctx, `SELECT id FROM issue WHERE id=$1 FOR UPDATE`, f.task.IssueID); err != nil {
				t.Fatal(err)
			}
			report := mustTerminalReport(t, f.identity)
			_, err = f.send(ctx, kind, report, nil)
			var busy *pgconn.PgError
			if !errors.As(err, &busy) || busy.Code != "55P03" {
				t.Fatalf("expected immediate issue-lock contention, got %v", err)
			}
			if report.Ack != nil || report.Replayed {
				t.Fatal("uncommitted report was acknowledged")
			}
			var status string
			f.fx.QueryRow(t, `SELECT status FROM agent_task_queue WHERE id=$1`, f.task.ID).Scan(&status)
			if status != "running" || f.fx.Count(t, `SELECT count(*) FROM terminal_report_receipt WHERE task_id=$1`, f.task.ID) != 0 {
				t.Fatal("contention left a partial terminal transition")
			}
			if err := writer.Commit(ctx); err != nil {
				t.Fatal(err)
			}
			retry := mustTerminalReport(t, f.identity)
			if _, err := f.send(ctx, kind, retry, nil); err != nil || retry.Ack == nil {
				t.Fatalf("same report did not recover after lock release: %v", err)
			}
		})
	}
}

func TestTerminalReportHookRollbackCannotAcknowledgeOrKeepComment(t *testing.T) {
	for _, kind := range []string{"complete", "fail", "retry"} {
		t.Run(kind, func(t *testing.T) {
			f := newReportFixture(t, kind == "retry")
			report := mustTerminalReport(t, f.identity)
			sentinel := errors.New("terminal handoff unavailable")
			_, err := f.send(context.Background(), kind, report, func(context.Context, pgx.Tx, *db.Queries, db.AgentTaskQueue) error { return sentinel })
			if !errors.Is(err, sentinel) || report.Ack != nil {
				t.Fatalf("failed tx returned err=%v ack=%+v", err, report.Ack)
			}
			var status string
			f.fx.QueryRow(t, `SELECT status FROM agent_task_queue WHERE id=$1`, f.task.ID).Scan(&status)
			if status != "running" {
				t.Fatalf("rollback left task %s", status)
			}
			for _, query := range []string{`SELECT count(*) FROM terminal_report_receipt WHERE task_id=$1`, `SELECT count(*) FROM comment WHERE source_task_id=$1`, `SELECT count(*) FROM agent_task_queue WHERE parent_task_id=$1`} {
				if got := f.fx.Count(t, query, f.task.ID); got != 0 {
					t.Fatalf("rollback retained %d rows for %s", got, query)
				}
			}
			accepted := mustTerminalReport(t, f.identity)
			if _, err := f.send(context.Background(), kind, accepted, nil); err != nil || accepted.Ack == nil {
				t.Fatalf("retry after rollback err=%v ack=%+v", err, accepted.Ack)
			}
		})
	}
}

func TestTerminalReportRejectsConflictingAndStaleResults(t *testing.T) {
	t.Run("different report or payload", func(t *testing.T) {
		f := newReportFixture(t, false)
		if _, err := f.send(context.Background(), "complete", mustTerminalReport(t, f.identity), nil); err != nil {
			t.Fatal(err)
		}
		for _, change := range []func(*protocol.TerminalReportIdentity){func(i *protocol.TerminalReportIdentity) { i.ReportID = util.UUIDToString(dbid.NewV7()) }, func(i *protocol.TerminalReportIdentity) { i.PayloadSHA256 = strings.Repeat("b", 64) }} {
			identity := f.identity
			change(&identity)
			report := mustTerminalReport(t, identity)
			if _, err := f.send(context.Background(), "fail", report, nil); !errors.Is(err, ErrTerminalReportConflict) || report.Ack != nil {
				t.Fatalf("conflict err=%v ack=%+v", err, report.Ack)
			}
		}
		var status string
		f.fx.QueryRow(t, `SELECT status FROM agent_task_queue WHERE id=$1`, f.task.ID).Scan(&status)
		if status != "completed" {
			t.Fatalf("conflict overwrote status: %s", status)
		}
	})
	t.Run("old dispatch", func(t *testing.T) {
		f := newReportFixture(t, false)
		f.fx.Exec(t, `UPDATE agent_task_queue SET dispatched_at=dispatched_at+interval '1 second' WHERE id=$1`, f.task.ID)
		report := mustTerminalReport(t, f.identity)
		if _, err := f.send(context.Background(), "complete", report, nil); !errors.Is(err, ErrTerminalReportStaleClaim) || report.Ack != nil {
			t.Fatalf("stale claim err=%v ack=%+v", err, report.Ack)
		}
		var status string
		f.fx.QueryRow(t, `SELECT status FROM agent_task_queue WHERE id=$1`, f.task.ID).Scan(&status)
		if status != "running" {
			t.Fatalf("stale claim changed status: %s", status)
		}
	})
	t.Run("legacy terminal has no receipt", func(t *testing.T) {
		f := newReportFixture(t, false)
		f.fx.Exec(t, `UPDATE agent_task_queue SET status='cancelled' WHERE id=$1`, f.task.ID)
		report := mustTerminalReport(t, f.identity)
		if _, err := f.send(context.Background(), "complete", report, nil); !errors.Is(err, ErrTerminalReportConflict) || report.Ack != nil {
			t.Fatalf("cancelled task err=%v ack=%+v", err, report.Ack)
		}
	})
}

func TestTerminalReportIDCannotAcknowledgeAnotherTask(t *testing.T) {
	first, second := newReportFixture(t, false), newReportFixture(t, false)
	if _, err := first.send(context.Background(), "complete", mustTerminalReport(t, first.identity), nil); err != nil {
		t.Fatal(err)
	}
	identity := second.identity
	identity.ReportID = first.identity.ReportID
	report := mustTerminalReport(t, identity)
	if _, err := second.send(context.Background(), "complete", report, nil); !errors.Is(err, ErrTerminalReportConflict) || report.Ack != nil {
		t.Fatalf("reused report ID err=%v ack=%+v", err, report.Ack)
	}
	var status string
	second.fx.QueryRow(t, `SELECT status FROM agent_task_queue WHERE id=$1`, second.task.ID).Scan(&status)
	if status != "running" {
		t.Fatalf("receipt conflict failed to roll back status: %s", status)
	}
	if got := second.fx.Count(t, `SELECT count(*) FROM comment WHERE source_task_id=$1`, second.task.ID); got != 0 {
		t.Fatalf("receipt conflict kept %d comments", got)
	}
}

func TestTerminalReportRequiresRealTransaction(t *testing.T) {
	f := newReportFixture(t, false)
	f.svc.TxStarter = nil
	report := mustTerminalReport(t, f.identity)
	if _, err := f.send(context.Background(), "complete", report, nil); err == nil || report.Ack != nil {
		t.Fatalf("missing transaction starter err=%v ack=%+v", err, report.Ack)
	}
	var status string
	f.fx.QueryRow(t, `SELECT status FROM agent_task_queue WHERE id=$1`, f.task.ID).Scan(&status)
	if status != "running" {
		t.Fatalf("nontransactional report changed status: %s", status)
	}
}
