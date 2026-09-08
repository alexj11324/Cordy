package service

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/orvilo-ai/orvilo/server/internal/events"
	"github.com/orvilo-ai/orvilo/server/internal/runtimeapps"
	dbfx "github.com/orvilo-ai/orvilo/server/internal/testutil"
	"github.com/orvilo-ai/orvilo/server/internal/util"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

func newIssueEnqueueFixture(t *testing.T) (*dbfx.Fixture, IssueCreateParams, string) {
	t.Helper()
	pool := newResolveOriginatorPool(t)
	f := dbfx.New(pool, "", "")
	suffix := time.Now().UnixNano()
	f.UserID = f.User(t, "Issue enqueue", fmt.Sprintf("issue-enqueue-%d@example.test", suffix))
	f.WorkspaceID = f.Workspace(t, "Issue enqueue", fmt.Sprintf("issue-enqueue-%d", suffix))
	f.Member(t, f.WorkspaceID, f.UserID, "owner")
	runtimeID := f.Runtime(t, "Issue enqueue runtime")
	agentID := f.Agent(t, "Issue enqueue agent", runtimeID)
	f.Cleanup(t, `DELETE FROM issue WHERE workspace_id = $1`, f.WorkspaceID)
	f.Cleanup(t, `DELETE FROM agent_task_queue WHERE agent_id = $1`, agentID)
	return f, IssueCreateParams{
		WorkspaceID: util.MustParseUUID(f.WorkspaceID), Title: "Atomic issue enqueue",
		Status: "todo", Priority: "medium", CreatorType: "member", CreatorID: util.MustParseUUID(f.UserID),
		ExecutorType: pgtype.Text{String: "agent", Valid: true}, ExecutorID: util.MustParseUUID(agentID),
	}, runtimeID
}

func TestCreateIssueTaskVisibleAtCommit(t *testing.T) {
	f, params, _ := newIssueEnqueueFixture(t)
	ctx := context.Background()
	q := db.New(f.Pool)
	bus := events.New()
	wakeup := &stubWakeup{}
	tasks := &TaskService{Queries: q, TxStarter: f.Pool, Bus: bus, Wakeup: wakeup}
	var eventOrder []string
	bus.SubscribeAll(func(e events.Event) { eventOrder = append(eventOrder, e.Type) })
	var visibleTasks int
	starter := &afterCommitTxStarter{pool: f.Pool, afterCommit: func() {
		visibleTasks = f.Count(t, `SELECT count(*) FROM agent_task_queue t JOIN issue i ON i.id = t.issue_id WHERE i.workspace_id = $1`, f.WorkspaceID)
		if len(eventOrder) != 0 || len(wakeup.calls) != 0 {
			t.Fatal("create published or woke the daemon before commit completed")
		}
	}}
	result, err := NewIssueService(q, starter, bus, nil, tasks).Create(ctx, params, IssueCreateOpts{})
	if err != nil {
		t.Fatalf("create: %v", err)
	}
	if visibleTasks != 1 {
		t.Fatalf("tasks visible at issue commit = %d, want 1", visibleTasks)
	}
	if !result.ExecutorTaskID.Valid {
		t.Fatal("successful agent create returned no executor task")
	}
	if len(eventOrder) != 2 || eventOrder[0] != protocol.EventIssueCreated || eventOrder[1] != protocol.EventTaskQueued {
		t.Fatalf("event order = %v, want issue:created then task:queued", eventOrder)
	}
	if len(wakeup.calls) != 1 || wakeup.calls[0].taskID != util.UUIDToString(result.ExecutorTaskID) {
		t.Fatalf("daemon wakeups = %+v", wakeup.calls)
	}
}

var errIssueTaskInsert = errors.New("injected issue task insert failure")

type issueTaskInsertFailure struct{ db.DBTX }

func (q issueTaskInsertFailure) QueryRow(ctx context.Context, query string, args ...any) pgx.Row {
	if strings.Contains(query, "INSERT INTO agent_task_queue") {
		return issueTaskFailedRow{}
	}
	return q.DBTX.QueryRow(ctx, query, args...)
}

type issueTaskFailedRow struct{}

func (issueTaskFailedRow) Scan(...any) error { return errIssueTaskInsert }

type issueTaskFailingTx struct{ pgx.Tx }

func (tx issueTaskFailingTx) QueryRow(ctx context.Context, query string, args ...any) pgx.Row {
	return issueTaskInsertFailure{DBTX: tx.Tx}.QueryRow(ctx, query, args...)
}

type issueTaskFailingTxStarter struct{ pool *pgxpool.Pool }

func (s issueTaskFailingTxStarter) Begin(ctx context.Context) (pgx.Tx, error) {
	tx, err := s.pool.Begin(ctx)
	if err != nil {
		return nil, err
	}
	return issueTaskFailingTx{Tx: tx}, nil
}

func TestCreateIssueTaskFailureRollsBackIssue(t *testing.T) {
	f, params, _ := newIssueEnqueueFixture(t)
	attachmentID := newIssueEnqueueAttachment(t, f)
	params.AttachmentIDs = []pgtype.UUID{util.MustParseUUID(attachmentID)}
	labelID := f.Insert(t, "issue_label", dbfx.Cols{"workspace_id": f.WorkspaceID, "name": "Atomic", "color": "#ff0000"})
	params.LabelIDs = []pgtype.UUID{util.MustParseUUID(labelID)}
	ctx := context.Background()
	q := db.New(issueTaskInsertFailure{DBTX: f.Pool})
	bus := events.New()
	wakeup := &stubWakeup{}
	tasks := &TaskService{Queries: q, TxStarter: f.Pool, Bus: bus, Wakeup: wakeup}
	var eventCount int
	bus.SubscribeAll(func(events.Event) { eventCount++ })
	_, err := NewIssueService(q, issueTaskFailingTxStarter{pool: f.Pool}, bus, nil, tasks).Create(ctx, params, IssueCreateOpts{})
	if !errors.Is(err, errIssueTaskInsert) {
		t.Fatalf("create error = %v, want task insert error", err)
	}
	if got := f.Count(t, `SELECT count(*) FROM issue WHERE workspace_id = $1`, f.WorkspaceID); got != 0 {
		t.Fatalf("issues after task insert failure = %d, want 0", got)
	}
	if got := f.Count(t, `SELECT issue_counter FROM workspace WHERE id = $1`, f.WorkspaceID); got != 0 {
		t.Fatalf("issue counter after rollback = %d, want 0", got)
	}
	if got := f.Count(t, `SELECT count(*) FROM issue_to_label WHERE label_id = $1`, labelID); got != 0 {
		t.Fatalf("label links after rollback = %d, want 0", got)
	}
	if got := f.Count(t, `SELECT count(*) FROM attachment WHERE id = $1 AND issue_id IS NULL`, attachmentID); got != 1 {
		t.Fatal("task failure did not restore the uploaded attachment to its unlinked state")
	}
	if eventCount != 0 || len(wakeup.calls) != 0 {
		t.Fatalf("rolled back create emitted %d events and %d wakeups", eventCount, len(wakeup.calls))
	}
}

func newIssueEnqueueAttachment(t *testing.T, f *dbfx.Fixture) string {
	t.Helper()
	return f.Insert(t, "attachment", dbfx.Cols{
		"workspace_id": f.WorkspaceID, "uploader_type": "member", "uploader_id": f.UserID,
		"filename": "input.txt", "url": "/test/input.txt", "content_type": "text/plain", "size_bytes": 1,
	})
}

func TestCreateIssueTaskClaimableAfterPostCommitInterruption(t *testing.T) {
	f, params, runtimeID := newIssueEnqueueFixture(t)
	attachmentID := newIssueEnqueueAttachment(t, f)
	params.AttachmentIDs = []pgtype.UUID{util.MustParseUUID(attachmentID)}
	q := db.New(f.Pool)
	bus := events.New()
	wakeup := &stubWakeup{}
	var eventCount int
	bus.SubscribeAll(func(events.Event) { eventCount++ })
	tasks := &TaskService{Queries: q, TxStarter: f.Pool, Bus: bus, Wakeup: wakeup}
	const interruption = "stop after durable issue commit"
	starter := &afterCommitTxStarter{pool: f.Pool, afterCommit: func() { panic(interruption) }}
	func() {
		defer func() {
			if got := recover(); got != interruption {
				t.Fatalf("post-commit interruption = %v", got)
			}
		}()
		_, _ = NewIssueService(q, starter, bus, nil, tasks).Create(context.Background(), params, IssueCreateOpts{})
	}()
	if eventCount != 0 || len(wakeup.calls) != 0 {
		t.Fatal("interrupted creator unexpectedly published a notification")
	}
	// A fresh service has none of the interrupted creator's in-memory state.
	// The real runtime poll must recover the task from the database alone.
	restartedBus := events.New()
	var recoveredEventOrder []string
	restartedBus.SubscribeAll(func(event events.Event) { recoveredEventOrder = append(recoveredEventOrder, event.Type) })
	restarted := &TaskService{Queries: db.New(f.Pool), TxStarter: f.Pool, Bus: restartedBus}
	task, err := restarted.ClaimTaskForRuntime(context.Background(), util.MustParseUUID(runtimeID))
	if err != nil || task == nil {
		t.Fatalf("poll after creator interruption = %v, %v; want durable task", task, err)
	}
	if got := f.Count(t, `SELECT count(*) FROM attachment WHERE id = $1 AND issue_id = $2`, attachmentID, task.IssueID); got != 1 {
		t.Fatal("claimed task is missing the input attachment at the commit boundary")
	}
	if task.OriginatorUserID != params.CreatorID || task.AccountableUserID != params.CreatorID ||
		task.OriginatorSource.String != "direct_human" || task.TriggerEvidenceRefID != task.IssueID {
		t.Fatalf("claimed task provenance = %+v", task)
	}
	if len(recoveredEventOrder) < 3 || recoveredEventOrder[0] != protocol.EventIssueCreated ||
		recoveredEventOrder[1] != protocol.EventTaskQueued || recoveredEventOrder[2] != protocol.EventTaskDispatch {
		t.Fatalf("recovered lifecycle order = %v, want issue:created, task:queued, task:dispatch", recoveredEventOrder)
	}
}

func TestCreateIssueExecutorAdmission(t *testing.T) {
	for _, tc := range []struct {
		name, status, runtimeStatus       string
		team, archived, unbound, unusable bool
		wantTask                          bool
	}{
		{name: "agent online", wantTask: true},
		{name: "agent offline waits", runtimeStatus: "offline", wantTask: true},
		{name: "backlog", status: "backlog"},
		{name: "custom backlog", status: "parked"},
		{name: "archived", archived: true},
		{name: "unbound", unbound: true},
		{name: "unusable", runtimeStatus: "offline", unusable: true},
		{name: "team online", team: true, wantTask: true},
		{name: "team offline", team: true, runtimeStatus: "offline"},
		{name: "team unusable", team: true, runtimeStatus: "offline", unusable: true},
		{name: "team backlog", team: true, status: "backlog"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			f, params, runtimeID := newIssueEnqueueFixture(t)
			if tc.status != "" {
				params.Status = tc.status
			}
			if tc.status == "parked" {
				f.Insert(t, "issue_status", dbfx.Cols{
					"workspace_id": f.WorkspaceID, "key": "parked", "name": "Parked", "description": "",
					"category": "backlog", "color": "#ff0000", "position": 1,
				})
			}
			if tc.runtimeStatus != "" {
				f.Exec(t, `UPDATE agent_runtime SET status = $1 WHERE id = $2`, tc.runtimeStatus, runtimeID)
			}
			if tc.archived {
				f.Exec(t, `UPDATE agent SET archived_at = now() WHERE id = $1`, params.ExecutorID)
			}
			if tc.unbound {
				f.Exec(t, `UPDATE agent SET runtime_id = NULL WHERE id = $1`, params.ExecutorID)
			}
			if tc.unusable {
				f.Exec(t, `UPDATE agent_runtime SET metadata = '{"offline_reason":{"code":"not_executable"}}' WHERE id = $1`, runtimeID)
			}
			if tc.team {
				params.ExecutorID = util.MustParseUUID(f.Team(t, "Atomic team", util.UUIDToString(params.ExecutorID)))
				params.ExecutorType.String = "team"
			}
			q := db.New(f.Pool)
			tasks := &TaskService{Queries: q, TxStarter: f.Pool, Bus: events.New()}
			result, err := NewIssueService(q, f.Pool, nil, nil, tasks).Create(context.Background(), params, IssueCreateOpts{})
			if err != nil {
				t.Fatalf("create: %v", err)
			}
			count := f.Count(t, `SELECT count(*) FROM agent_task_queue WHERE issue_id = $1`, result.Issue.ID)
			if (count == 1) != tc.wantTask || count > 1 {
				t.Fatalf("task count = %d, want task %v", count, tc.wantTask)
			}
			if tc.team && tc.wantTask {
				if got := f.Count(t, `SELECT count(*) FROM agent_task_queue WHERE issue_id = $1 AND team_id = $2 AND is_leader_task`, result.Issue.ID, params.ExecutorID); got != 1 {
					t.Fatal("team task lost its leader role or team id")
				}
			}
			if tc.unusable {
				wantNotices := 1
				if tc.team {
					wantNotices = 0
				}
				if got := f.Count(t, `SELECT count(*) FROM comment WHERE issue_id = $1 AND author_type = 'system'`, result.Issue.ID); got != wantNotices {
					t.Fatalf("unusable executor notices = %d, want %d", got, wantNotices)
				}
			}
		})
	}
}

type issueOverlayFunc func(context.Context, pgtype.UUID, db.Agent) (runtimeapps.MCPOverlayResult, error)

func (f issueOverlayFunc) BuildTaskOverlay(ctx context.Context, originator pgtype.UUID, agent db.Agent) (runtimeapps.MCPOverlayResult, error) {
	return f(ctx, originator, agent)
}

type issueBeforeBeginTxStarter struct {
	pool        *pgxpool.Pool
	beforeBegin func()
}

func (s issueBeforeBeginTxStarter) Begin(ctx context.Context) (pgx.Tx, error) {
	s.beforeBegin()
	return s.pool.Begin(ctx)
}

func TestCreateIssueOverlayHydratesAfterTransactionalAdmission(t *testing.T) {
	for _, fails := range []bool{false, true} {
		t.Run(fmt.Sprintf("overlay fails %v", fails), func(t *testing.T) {
			f, params, _ := newIssueEnqueueFixture(t)
			q := db.New(f.Pool)
			committed, calls := false, 0
			builder := issueOverlayFunc(func(_ context.Context, user pgtype.UUID, agent db.Agent) (runtimeapps.MCPOverlayResult, error) {
				calls++
				if !committed {
					t.Fatal("external overlay called before issue admission committed")
				}
				if user != params.CreatorID || agent.ID != params.ExecutorID {
					t.Fatal("overlay prepared for wrong human or executor")
				}
				if fails {
					return runtimeapps.MCPOverlayResult{}, errors.New("optional overlay unavailable")
				}
				return runtimeapps.MCPOverlayResult{MCPOverlay: json.RawMessage(`{"mcpServers":{"fixture":{"url":"https://fixture.example"}}}`)}, nil
			})
			tasks := &TaskService{Queries: q, TxStarter: f.Pool, Bus: events.New(), Composio: builder, FeatureFlags: composioMCPAppsTestFlags(true)}
			starter := &afterCommitTxStarter{pool: f.Pool, afterCommit: func() { committed = true }}
			result, err := NewIssueService(q, starter, nil, nil, tasks).Create(context.Background(), params, IssueCreateOpts{})
			if err != nil {
				t.Fatalf("create with optional overlay: %v", err)
			}
			task, err := q.GetAgentTask(context.Background(), result.ExecutorTaskID)
			if err != nil {
				t.Fatalf("load task: %v", err)
			}
			if calls != 1 || (len(task.RuntimeMcpOverlay) > 0) == fails {
				t.Fatalf("overlay calls = %d, persisted overlay = %s", calls, task.RuntimeMcpOverlay)
			}
		})
	}
}

func TestCreateDeferredIssueOverlayRemainsPostCommit(t *testing.T) {
	f, params, _ := newIssueEnqueueFixture(t)
	q := db.New(f.Pool)
	committed, calls := false, 0
	builder := issueOverlayFunc(func(context.Context, pgtype.UUID, db.Agent) (runtimeapps.MCPOverlayResult, error) {
		calls++
		if !committed {
			t.Error("deferred issue overlay fetched before commit")
		}
		return runtimeapps.MCPOverlayResult{MCPOverlay: json.RawMessage(`{"mcpServers":{"fixture":{"url":"https://fixture.example"}}}`)}, nil
	})
	tasks := &TaskService{Queries: q, TxStarter: f.Pool, Bus: events.New(), Composio: builder, FeatureFlags: composioMCPAppsTestFlags(true)}
	starter := &afterCommitTxStarter{pool: f.Pool, afterCommit: func() { committed = true }}
	_, err := NewIssueService(q, starter, nil, nil, tasks).Create(context.Background(), params, IssueCreateOpts{ExecutorRunFireAt: time.Now().Add(time.Minute)})
	if err != nil {
		t.Fatalf("create deferred issue: %v", err)
	}
	if calls != 1 {
		t.Fatalf("deferred overlay calls = %d, want 1", calls)
	}
}

func TestCreateIssueOverlayUsesTransactionOriginator(t *testing.T) {
	f, params, runtimeID := newIssueEnqueueFixture(t)
	originID := f.Task(t, util.UUIDToString(params.ExecutorID), dbfx.Cols{"runtime_id": runtimeID, "status": "completed"})
	otherUserID := f.User(t, "Other originator", fmt.Sprintf("other-originator-%d@example.test", time.Now().UnixNano()))
	f.Member(t, f.WorkspaceID, otherUserID, "member")
	otherUser := util.MustParseUUID(otherUserID)
	params.CreatorType, params.CreatorID = "agent", params.ExecutorID
	params.OriginType = pgtype.Text{String: "agent_create", Valid: true}
	params.OriginID = util.MustParseUUID(originID)
	q := db.New(f.Pool)
	builder := &stubOverlayBuilder{resp: json.RawMessage(`{"mcpServers":{"private":{"url":"https://original-human.example"}}}`)}
	tasks := &TaskService{Queries: q, TxStarter: f.Pool, Bus: events.New(), Composio: builder, FeatureFlags: composioMCPAppsTestFlags(true)}
	starter := issueBeforeBeginTxStarter{pool: f.Pool, beforeBegin: func() {
		f.Exec(t, `UPDATE agent_task_queue SET originator_user_id = $1, accountable_user_id = $1 WHERE id = $2`, otherUser, params.OriginID)
	}}
	result, err := NewIssueService(q, starter, nil, nil, tasks).Create(context.Background(), params, IssueCreateOpts{})
	if err != nil {
		t.Fatalf("create after attribution change: %v", err)
	}
	task, err := q.GetAgentTask(context.Background(), result.ExecutorTaskID)
	if err != nil {
		t.Fatalf("load task: %v", err)
	}
	if builder.calls != 1 || builder.lastUser != otherUser {
		t.Fatal("overlay did not use the transaction-time originator")
	}
	if task.OriginatorUserID != otherUser || task.AccountableUserID != otherUser || task.DelegatedFromTaskID != params.OriginID {
		t.Fatal("task did not retain transaction-time delegated attribution")
	}
	if len(task.RuntimeMcpOverlay) == 0 {
		t.Fatal("task attributed to the transaction-time human lost its overlay")
	}
}

func TestCreateIssueAttributionPolicy(t *testing.T) {
	for _, failClosed := range []bool{false, true} {
		t.Run(fmt.Sprintf("fail closed %v", failClosed), func(t *testing.T) {
			f, params, _ := newIssueEnqueueFixture(t)
			params.CreatorType, params.CreatorID = "agent", params.ExecutorID
			f.Exec(t, `UPDATE workspace SET attribution_fail_closed = $1 WHERE id = $2`, failClosed, params.WorkspaceID)
			q := db.New(f.Pool)
			tasks := &TaskService{Queries: q, TxStarter: f.Pool, Bus: events.New()}
			result, err := NewIssueService(q, f.Pool, nil, nil, tasks).Create(context.Background(), params, IssueCreateOpts{})
			if failClosed {
				if !errors.Is(err, ErrAttributionFailClosed) {
					t.Fatalf("create error = %v, want attribution refusal", err)
				}
				if got := f.Count(t, `SELECT count(*) FROM issue WHERE workspace_id = $1`, params.WorkspaceID); got != 0 {
					t.Fatal("refused required executor left a committed issue")
				}
				return
			}
			if err != nil {
				t.Fatalf("create with owner fallback: %v", err)
			}
			task, err := q.GetAgentTask(context.Background(), result.ExecutorTaskID)
			if err != nil {
				t.Fatalf("load task: %v", err)
			}
			if task.OriginatorUserID.Valid || task.AccountableUserID != util.MustParseUUID(f.UserID) || task.OriginatorSource.String != "owner_fallback" {
				t.Fatal("owner fallback changed authority or lost its accountable human")
			}
		})
	}
}

type issueCreatePIDStarter struct {
	pool *pgxpool.Pool
	pid  chan uint32
}

func (s issueCreatePIDStarter) Begin(ctx context.Context) (pgx.Tx, error) {
	tx, err := s.pool.Begin(ctx)
	if err == nil {
		s.pid <- tx.Conn().PgConn().PID()
	}
	return tx, err
}

func TestCreateIssueSourceContextUsesAttachmentBeforeIssueLockOrder(t *testing.T) {
	f, params, _ := newIssueEnqueueFixture(t)
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	sourceID := f.Issue(t, "Source issue")
	f.Exec(t, `UPDATE workspace SET issue_counter=1 WHERE id=$1`, f.WorkspaceID)
	commentID := f.Comment(t, sourceID, "Anchor")
	attachmentID := newIssueEnqueueAttachment(t, f)
	q := db.New(f.Pool)
	build, err := BuildSourceContext(ctx, q, params.WorkspaceID, util.MustParseUUID(commentID))
	if err != nil {
		t.Fatal(err)
	}
	capture, err := PrepareSourceContextCapture(build, util.MustParseUUID(commentID), params.WorkspaceID, params.CreatorID, time.Now().UTC(), nil)
	if err != nil {
		t.Fatal(err)
	}
	f.Cleanup(t, `DELETE FROM issue_source_context WHERE workspace_id=$1`, f.WorkspaceID)
	params.SourceContext = &capture
	params.ParentIssueID = util.MustParseUUID(sourceID)
	params.AttachmentIDs = []pgtype.UUID{util.MustParseUUID(attachmentID)}
	editTx, err := f.Pool.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer editTx.Rollback(context.Background())
	editQueries := q.WithTx(editTx)
	if _, err := editQueries.LockAttachmentsForIssueLink(ctx, db.LockAttachmentsForIssueLinkParams{WorkspaceID: params.WorkspaceID, AttachmentIds: params.AttachmentIDs}); err != nil {
		t.Fatal(err)
	}
	pid := make(chan uint32, 1)
	createResult := make(chan error, 1)
	starter := issueCreatePIDStarter{pool: f.Pool, pid: pid}
	tasks := &TaskService{Queries: q, TxStarter: f.Pool, Bus: events.New()}
	go func() {
		_, err := NewIssueService(q, starter, nil, nil, tasks).Create(ctx, params, IssueCreateOpts{})
		createResult <- err
	}()
	var createPID uint32
	select {
	case createPID = <-pid:
	case err := <-createResult:
		t.Fatalf("create returned before starting transaction: %v", err)
	case <-ctx.Done():
		t.Fatal(ctx.Err())
	}
	// Wait for PostgreSQL itself to prove create is waiting for our attachment
	// lock. This schedule does not depend on a sleep or goroutine timing.
	for {
		var waiting bool
		if err := f.Pool.QueryRow(ctx, `SELECT $1::int=ANY(pg_blocking_pids($2::int))`, int32(editTx.Conn().PgConn().PID()), int32(createPID)).Scan(&waiting); err != nil {
			t.Fatal(err)
		}
		if waiting {
			break
		}
		select {
		case err := <-createResult:
			t.Fatalf("create returned before lock wait: %v", err)
		case <-ctx.Done():
			t.Fatal(ctx.Err())
		default:
		}
	}
	// This is the next lock taken by updateIssueAtomically. Before the fix,
	// create already held this source issue and the two transactions deadlocked.
	_, editErr := editQueries.LockIssueForDescriptionUpdate(ctx, db.LockIssueForDescriptionUpdateParams{ID: params.ParentIssueID, WorkspaceID: params.WorkspaceID})
	if err := editTx.Rollback(context.Background()); err != nil {
		t.Fatal(err)
	}
	createErr := <-createResult
	if editErr != nil || createErr != nil {
		t.Fatalf("concurrent source edit/create failed: edit=%v create=%v", editErr, createErr)
	}
}

func TestCreateIssueFencesWorkspaceDeletionBeforeAttachmentLocks(t *testing.T) {
	f, params, _ := newIssueEnqueueFixture(t)
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	attachmentID := newIssueEnqueueAttachment(t, f)
	params.AttachmentIDs = []pgtype.UUID{util.MustParseUUID(attachmentID)}
	q := db.New(f.Pool)
	teardown, err := f.Pool.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer teardown.Rollback(context.Background())
	if _, err := q.WithTx(teardown).LockWorkspaceForDelete(ctx, params.WorkspaceID); err != nil {
		t.Fatal(err)
	}
	pid := make(chan uint32, 1)
	result := make(chan error, 1)
	tasks := &TaskService{Queries: q, TxStarter: f.Pool, Bus: events.New()}
	go func() {
		_, err := NewIssueService(q, issueCreatePIDStarter{pool: f.Pool, pid: pid}, nil, nil, tasks).Create(ctx, params, IssueCreateOpts{})
		result <- err
	}()
	var createPID uint32
	select {
	case createPID = <-pid:
	case err := <-result:
		t.Fatalf("create returned before transaction: %v", err)
	case <-ctx.Done():
		t.Fatal(ctx.Err())
	}
	for {
		var waiting bool
		if err := f.Pool.QueryRow(ctx, `SELECT $1::int=ANY(pg_blocking_pids($2::int))`, int32(teardown.Conn().PgConn().PID()), int32(createPID)).Scan(&waiting); err != nil {
			t.Fatal(err)
		}
		if waiting {
			break
		}
		select {
		case err := <-result:
			t.Fatalf("create returned before lock wait: %v", err)
		case <-ctx.Done():
			t.Fatal(ctx.Err())
		default:
		}
	}
	// The teardown owner must be able to remove its attachment while create
	// waits for the workspace. Before the fence, create held the attachment
	// while waiting for the issue counter, producing the opposite lock order.
	_, deleteErr := teardown.Exec(ctx, `DELETE FROM attachment WHERE id=$1`, attachmentID)
	if err := teardown.Rollback(context.Background()); err != nil {
		t.Fatal(err)
	}
	createErr := <-result
	if deleteErr != nil || createErr != nil {
		t.Fatalf("workspace teardown/create lock cycle: delete=%v create=%v", deleteErr, createErr)
	}
}
