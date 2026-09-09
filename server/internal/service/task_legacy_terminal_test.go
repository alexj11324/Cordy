package service

import (
	"context"
	"encoding/json"
	"errors"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/orvilo-ai/orvilo/server/internal/events"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
	"github.com/orvilo-ai/orvilo/server/internal/util"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

func (f reportFixture) sendLegacy(ctx context.Context, kind string, hook TerminalTaskTxHook) (*db.AgentTaskQueue, error) {
	if kind == "complete" {
		return f.svc.CompleteTaskWithTerminalHook(ctx, f.task.ID, []byte(`{"output":"Implemented the terminal report result."}`), "", "", "", false, "", "", hook)
	}
	reason := "agent_error.process_failure"
	if kind == "retry" {
		reason = "timeout"
	}
	return f.svc.FailTaskWithTerminalHook(ctx, f.task.ID, "worker failed", "", "", "", reason, false, "", "", hook)
}

func TestLegacyTerminalCommentCommitsWithTaskAndReplaysOnce(t *testing.T) {
	for _, kind := range []string{"complete", "fail", "retry"} {
		t.Run(kind, func(t *testing.T) {
			f := newReportFixture(t, kind == "retry")
			ctx := context.Background()
			wantComments := 1
			if kind == "retry" {
				wantComments = 0
			}
			var published atomic.Int32
			f.svc.Bus.Subscribe(protocol.EventCommentCreated, func(events.Event) {
				published.Add(1)
				var status string
				f.fx.QueryRow(t, `SELECT status FROM agent_task_queue WHERE id=$1`, f.task.ID).Scan(&status)
				if status == "running" {
					t.Error("comment was published before the terminal transaction committed")
				}
			})
			var hooks int
			hook := func(ctx context.Context, tx pgx.Tx, _ *db.Queries, task db.AgentTaskQueue) error {
				hooks++
				var comments int
				if err := tx.QueryRow(ctx, `SELECT count(*) FROM comment WHERE source_task_id=$1`, task.ID).Scan(&comments); err != nil {
					return err
				}
				if comments != wantComments || published.Load() != 0 {
					t.Errorf("inside terminal transaction: comments=%d want %d, published=%d want 0", comments, wantComments, published.Load())
				}
				return nil
			}
			for range 2 {
				if _, err := f.sendLegacy(ctx, kind, hook); err != nil {
					t.Fatal(err)
				}
			}
			if hooks != 1 || published.Load() != int32(wantComments) {
				t.Fatalf("hooks=%d published=%d, want 1 and %d", hooks, published.Load(), wantComments)
			}
			if comments := f.fx.Count(t, `SELECT count(*) FROM comment WHERE source_task_id=$1`, f.task.ID); comments != wantComments {
				t.Fatalf("comments=%d want %d", comments, wantComments)
			}
			if receipts := f.fx.Count(t, `SELECT count(*) FROM terminal_report_receipt WHERE task_id=$1`, f.task.ID); receipts != 0 {
				t.Fatalf("legacy terminal delivery created %d fenced receipts", receipts)
			}
			wantRetries := 1 - wantComments
			if retries := f.fx.Count(t, `SELECT count(*) FROM agent_task_queue WHERE parent_task_id=$1`, f.task.ID); retries != wantRetries {
				t.Fatalf("retry children=%d want %d", retries, wantRetries)
			}
		})
	}
}

func TestLegacyTerminalHookErrorCannotBecomeIdempotentSuccess(t *testing.T) {
	for _, kind := range []string{"complete", "fail", "retry"} {
		for _, hookErr := range []error{errors.New("terminal handoff unavailable"), pgx.ErrNoRows} {
			t.Run(kind+"/"+hookErr.Error(), func(t *testing.T) {
				f := newReportFixture(t, kind == "retry")
				ctx := context.Background()
				var published atomic.Int32
				f.svc.Bus.SubscribeAll(func(events.Event) { published.Add(1) })
				task, err := f.sendLegacy(ctx, kind, func(context.Context, pgx.Tx, *db.Queries, db.AgentTaskQueue) error { return hookErr })
				if !errors.Is(err, hookErr) || task != nil {
					t.Fatalf("failed transaction returned task=%+v error=%v, want nil and %v", task, err, hookErr)
				}
				stored, err := f.svc.Queries.GetAgentTask(ctx, f.task.ID)
				if err != nil {
					t.Fatal(err)
				}
				if stored.Status != "running" || stored.CompletedAt.Valid || stored.Error.Valid || len(stored.Result) != 0 {
					t.Fatalf("hook failure changed task: %+v", stored)
				}
				for _, query := range []string{`SELECT count(*) FROM comment WHERE source_task_id=$1`, `SELECT count(*) FROM terminal_report_receipt WHERE task_id=$1`, `SELECT count(*) FROM agent_task_queue WHERE parent_task_id=$1`} {
					if rows := f.fx.Count(t, query, f.task.ID); rows != 0 {
						t.Fatalf("rollback retained %d rows for %s", rows, query)
					}
				}
				if published.Load() != 0 {
					t.Fatalf("failed terminal transaction published %d events", published.Load())
				}
				if _, err := f.sendLegacy(ctx, kind, nil); err != nil {
					t.Fatalf("redelivery after rollback failed: %v", err)
				}
			})
		}
	}
}

func TestLegacyTerminalWaitsForIssueBeforeTaskAndChatLocks(t *testing.T) {
	for _, kind := range []string{"complete", "fail"} {
		t.Run(kind, func(t *testing.T) {
			f := newReportFixture(t, false)
			// Current producers use issue-only or chat-only tasks. Giving this
			// lock-order probe both references also proves chat is not locked
			// before legacy issue admission, independent of producer routing.
			chatID := f.fx.ChatSession(t, util.UUIDToString(f.task.AgentID))
			f.fx.Exec(t, `UPDATE agent_task_queue SET chat_session_id=$2 WHERE id=$1`, f.task.ID, chatID)
			f.fx.Cleanup(t, `DELETE FROM chat_message WHERE chat_session_id=$1`, chatID)
			ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			defer cancel()
			writer, err := f.fx.Pool.Begin(ctx)
			if err != nil {
				t.Fatal(err)
			}
			defer writer.Rollback(context.Background())
			if _, err := writer.Exec(ctx, `SELECT id FROM issue WHERE id=$1 FOR UPDATE`, f.task.IssueID); err != nil {
				t.Fatal(err)
			}
			terminalConn, err := f.fx.Pool.Acquire(ctx)
			if err != nil {
				t.Fatal(err)
			}
			defer terminalConn.Release()
			f.svc.TxStarter = terminalConn
			var hooks atomic.Int32
			hook := func(context.Context, pgx.Tx, *db.Queries, db.AgentTaskQueue) error { hooks.Add(1); return nil }
			result := make(chan error, 1)
			finished := make(chan struct{})
			go func() {
				_, err := f.sendLegacy(ctx, kind, hook)
				result <- err
				close(finished)
			}()
			defer func() {
				cancel()
				_ = writer.Rollback(context.Background())
				<-finished
			}()
			for {
				var waiting bool
				if err := f.fx.Pool.QueryRow(ctx, `SELECT $1::int=ANY(pg_blocking_pids($2::int))`, int32(writer.Conn().PgConn().PID()), int32(terminalConn.Conn().PgConn().PID())).Scan(&waiting); err != nil {
					t.Fatal(err)
				}
				if waiting {
					break
				}
				select {
				case err := <-result:
					t.Fatalf("legacy terminal returned before waiting for issue writer: %v", err)
				case <-ctx.Done():
					t.Fatal(ctx.Err())
				default:
				}
			}
			// Rerun/delete writers can acquire the task after locking the issue;
			// admission must not hold either of the later terminal locks yet.
			if _, err := writer.Exec(ctx, `SELECT id FROM agent_task_queue WHERE id=$1 FOR UPDATE NOWAIT`, f.task.ID); err != nil {
				t.Fatalf("terminal held task while waiting for issue: %v", err)
			}
			if _, err := writer.Exec(ctx, `SELECT id FROM chat_session WHERE id=$1 FOR UPDATE NOWAIT`, chatID); err != nil {
				t.Fatalf("terminal held chat while waiting for issue: %v", err)
			}
			var status string
			f.fx.QueryRow(t, `SELECT status FROM agent_task_queue WHERE id=$1`, f.task.ID).Scan(&status)
			if status != "running" || f.fx.Count(t, `SELECT count(*) FROM comment WHERE source_task_id=$1`, f.task.ID) != 0 {
				t.Fatal("contention left a partial terminal transition")
			}
			if err := writer.Commit(ctx); err != nil {
				t.Fatal(err)
			}
			if err := <-result; err != nil {
				t.Fatalf("terminal did not finish after writer committed: %v", err)
			}
			if _, err := f.sendLegacy(ctx, kind, hook); err != nil {
				t.Fatalf("idempotent legacy replay failed: %v", err)
			}
			wantStatus := "completed"
			if kind == "fail" {
				wantStatus = "failed"
			}
			f.fx.QueryRow(t, `SELECT status FROM agent_task_queue WHERE id=$1`, f.task.ID).Scan(&status)
			if status != wantStatus || hooks.Load() != 1 || f.fx.Count(t, `SELECT count(*) FROM comment WHERE source_task_id=$1`, f.task.ID) != 1 {
				t.Fatalf("terminal outcome was not committed once: status=%s hooks=%d", status, hooks.Load())
			}
		})
	}
}

func TestLegacyCompletePreservesFallbackSuppression(t *testing.T) {
	for _, scenario := range []string{"already commented", "no action", "trivial threaded output", "empty output", "unstructured result", "nil result"} {
		t.Run(scenario, func(t *testing.T) {
			f := newReportFixture(t, false)
			result := []byte(`{"output":"Implemented the terminal report result."}`)
			switch scenario {
			case "already commented":
				// Use the task's timestamp so host/database clock skew cannot put
				// this in-run comment before the run it is meant to cover.
				f.fx.Comment(t, util.UUIDToString(f.task.IssueID), "The implementation is ready.", testutil.Cols{"author_type": "agent", "author_id": util.UUIDToString(f.task.AgentID), "created_at": f.task.StartedAt.Time})
			case "no action":
				details, err := json.Marshal(map[string]string{"outcome": "no_action", "task_id": util.UUIDToString(f.task.ID)})
				if err != nil {
					t.Fatal(err)
				}
				f.fx.Insert(t, "activity_log", testutil.Cols{"workspace_id": f.fx.WorkspaceID, "issue_id": util.UUIDToString(f.task.IssueID), "actor_type": "agent", "actor_id": util.UUIDToString(f.task.AgentID), "action": "team_leader_evaluated", "details": string(details)})
			case "trivial threaded output":
				triggerID := f.fx.Comment(t, util.UUIDToString(f.task.IssueID), "Please implement this.")
				f.fx.Exec(t, `UPDATE agent_task_queue SET trigger_comment_id=$2 WHERE id=$1`, f.task.ID, triggerID)
				result = []byte(`{"output":"Done."}`)
			case "empty output":
				result = []byte(`{"output":""}`)
			case "unstructured result":
				result = []byte(`{"output":42}`)
			case "nil result":
				result = nil
			}
			task, err := f.svc.CompleteTask(context.Background(), f.task.ID, result, "", "", "", false, "", "")
			if err != nil || task == nil || task.Status != "completed" {
				t.Fatalf("completion failed: task=%+v err=%v", task, err)
			}
			if comments := f.fx.Count(t, `SELECT count(*) FROM comment WHERE source_task_id=$1`, f.task.ID); comments != 0 {
				t.Fatalf("suppressed fallback produced %d comments", comments)
			}
		})
	}
}

type failTerminalCommentTxStarter struct {
	TxStarter
	err error
}

func (s failTerminalCommentTxStarter) Begin(ctx context.Context) (pgx.Tx, error) {
	tx, err := s.TxStarter.Begin(ctx)
	if err != nil {
		return nil, err
	}
	return failTerminalCommentTx{Tx: tx, err: s.err}, nil
}

type failTerminalCommentTx struct {
	pgx.Tx
	err error
}

func (tx failTerminalCommentTx) QueryRow(ctx context.Context, sql string, args ...any) pgx.Row {
	if strings.HasPrefix(sql, "-- name: CreateComment :one") {
		return &mockRow{err: tx.err}
	}
	return tx.Tx.QueryRow(ctx, sql, args...)
}

func TestLegacyTerminalCommentWriteFailureRollsBackTask(t *testing.T) {
	for _, kind := range []string{"complete", "fail"} {
		t.Run(kind, func(t *testing.T) {
			f := newReportFixture(t, false)
			f.svc.TxStarter = failTerminalCommentTxStarter{TxStarter: f.fx.Pool, err: pgx.ErrNoRows}
			var published atomic.Int32
			f.svc.Bus.SubscribeAll(func(events.Event) { published.Add(1) })
			task, err := f.sendLegacy(context.Background(), kind, nil)
			if !errors.Is(err, pgx.ErrNoRows) || task != nil {
				t.Fatalf("comment write failure returned task=%+v err=%v", task, err)
			}
			var status string
			f.fx.QueryRow(t, `SELECT status FROM agent_task_queue WHERE id=$1`, f.task.ID).Scan(&status)
			if status != "running" || f.fx.Count(t, `SELECT count(*) FROM comment WHERE source_task_id=$1`, f.task.ID) != 0 || published.Load() != 0 {
				t.Fatalf("comment write failure was not atomic: status=%s published=%d", status, published.Load())
			}
			f.svc.TxStarter = f.fx.Pool
			if _, err := f.sendLegacy(context.Background(), kind, nil); err != nil {
				t.Fatalf("redelivery after comment failure failed: %v", err)
			}
		})
	}
}

func TestLegacyTerminalCASMissOnNonterminalTaskIsAnError(t *testing.T) {
	for _, kind := range []string{"complete", "fail"} {
		t.Run(kind, func(t *testing.T) {
			f := newReportFixture(t, false)
			f.fx.Exec(t, `UPDATE agent_task_queue SET status='queued' WHERE id=$1`, f.task.ID)
			task, err := f.sendLegacy(context.Background(), kind, nil)
			if !errors.Is(err, pgx.ErrNoRows) || task != nil {
				t.Fatalf("nonterminal CAS miss returned task=%+v err=%v", task, err)
			}
		})
	}
}
