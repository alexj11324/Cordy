package handler

import (
	"context"
	"errors"
	"net/http"
	"strings"
	"testing"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	obsmetrics "github.com/patchbay-ai/patchbay/server/internal/metrics"
	"github.com/patchbay-ai/patchbay/server/internal/testutil"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/prometheus/client_golang/prometheus"
)

type failChatInputQueryDB struct {
	delegate db.DBTX
}

func (f *failChatInputQueryDB) Exec(ctx context.Context, query string, args ...interface{}) (pgconn.CommandTag, error) {
	return f.delegate.Exec(ctx, query, args...)
}

func (f *failChatInputQueryDB) Query(ctx context.Context, query string, args ...interface{}) (pgx.Rows, error) {
	if strings.Contains(query, "-- name: ListChatInputMessages") {
		return nil, errors.New("injected chat input load failure")
	}
	return f.delegate.Query(ctx, query, args...)
}

func (f *failChatInputQueryDB) QueryRow(ctx context.Context, query string, args ...interface{}) pgx.Row {
	return f.delegate.QueryRow(ctx, query, args...)
}

func TestChatSessionResumeFallbackNeeded(t *testing.T) {
	tests := []struct {
		name           string
		priorSessionID string
		priorWorkDir   string
		want           bool
	}{
		{name: "both present", priorSessionID: "session", priorWorkDir: "/work", want: false},
		{name: "session missing", priorWorkDir: "/work", want: true},
		{name: "workdir missing", priorSessionID: "session", want: true},
		{name: "both missing", want: true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := chatSessionResumeFallbackNeeded(tt.priorSessionID, tt.priorWorkDir); got != tt.want {
				t.Fatalf("chatSessionResumeFallbackNeeded(%q, %q) = %v, want %v", tt.priorSessionID, tt.priorWorkDir, got, tt.want)
			}
		})
	}
}

func TestClaimTaskChatCompletePointerSkipsSessionFallbackQuery(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	ctx := context.Background()
	agentID, runtimeID, daemonID := createRuntimeGuardAgent(t, ctx)

	chatSessionID := dbfx.ChatSession(t, agentID, testutil.Cols{
		"title":      "complete resume pointer",
		"session_id": "pointer-session",
		"work_dir":   "/pointer-workdir",
		"runtime_id": runtimeID,
	})
	dbfx.Insert(t, "chat_message", testutil.Cols{
		"chat_session_id": chatSessionID,
		"role":            "user",
		"content":         "keep the direct pointer",
	})
	dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id":      runtimeID,
		"chat_session_id": chatSessionID,
		"priority":        1000,
	})

	claimMetrics := obsmetrics.NewBusinessMetrics()
	h := *testHandler
	h.Metrics = claimMetrics

	req := newDaemonTokenRequest(http.MethodPost, "/api/daemon/runtimes/"+runtimeID+"/claim", nil,
		testWorkspaceID, daemonID)
	req = testutil.WithURLParams(req, "runtimeId", runtimeID)
	w := testutil.Call(t, h.ClaimTaskByRuntime, req).Want(http.StatusOK)
	var resp struct {
		Task *claimRuntimeGuardTask `json:"task"`
	}
	w.JSON(&resp)
	if resp.Task == nil {
		t.Fatal("expected a claimed task")
	}
	if resp.Task.PriorSessionID != "pointer-session" || resp.Task.PriorWorkDir != "/pointer-workdir" {
		t.Fatalf("claim pointer = (%q, %q), want direct chat-session pointer", resp.Task.PriorSessionID, resp.Task.PriorWorkDir)
	}

	registry := prometheus.NewRegistry()
	registry.MustRegister(claimMetrics.Collectors()...)
	families, err := registry.Gather()
	if err != nil {
		t.Fatalf("gather claim metrics: %v", err)
	}
	seenRolloutQuery := false
	for _, family := range families {
		switch family.GetName() {
		case "patchbay_chat_claim_session_fallback_needed_total":
			if len(family.Metric) != 1 || family.Metric[0].GetCounter().GetValue() != 0 {
				t.Fatalf("complete pointer unexpectedly needed session fallback: %v", family)
			}
		case "patchbay_chat_claim_session_fallback_result_total":
			t.Fatalf("complete pointer unexpectedly emitted a session fallback result: %v", family)
		case "patchbay_chat_claim_resume_query_duration_seconds":
			for _, metric := range family.Metric {
				for _, label := range metric.Label {
					if label.GetName() != "query" {
						continue
					}
					switch label.GetValue() {
					case "rollout_missing":
						seenRolloutQuery = true
					case "last_session":
						t.Fatal("complete pointer unexpectedly ran GetLastChatTaskSession")
					}
				}
			}
		}
	}
	if !seenRolloutQuery {
		t.Fatal("independent rollout-missing query was not observed")
	}
}

func TestClaimOldAgentTaskDoesNotBorrowNewHubAgentsResumePointer(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	agentID, runtimeID, daemonID := createRuntimeGuardAgent(t, ctx)
	selectedAgent := dbfx.Agent(t, "new Hub Agent", runtimeID)
	chatSessionID := dbfx.ChatSession(t, selectedAgent, testutil.Cols{
		"title": "Hub Agent switch", "session_id": "new-agent-session", "work_dir": "/new-agent-directory", "runtime_id": runtimeID,
	})
	dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id": runtimeID, "chat_session_id": chatSessionID, "status": "cancelled",
		"session_id": "old-agent-session", "work_dir": "/old-agent-directory", "completed_at": testutil.Raw("now() - interval '1 minute'"),
	})
	dbfx.Insert(t, "chat_message", testutil.Cols{
		"chat_session_id": chatSessionID, "role": "user", "content": "the old Agent's pending turn",
	})
	dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id": runtimeID, "chat_session_id": chatSessionID, "priority": 1000,
	})
	req := newDaemonTokenRequest(http.MethodPost, "/api/daemon/runtimes/"+runtimeID+"/claim", nil, testWorkspaceID, daemonID)
	req = testutil.WithURLParams(req, "runtimeId", runtimeID)
	var response struct {
		Task *claimRuntimeGuardTask `json:"task"`
	}
	testutil.Call(t, testHandler.ClaimTaskByRuntime, req).Want(http.StatusOK).JSON(&response)
	if response.Task == nil || response.Task.PriorSessionID != "old-agent-session" || response.Task.PriorWorkDir != "/old-agent-directory" {
		t.Fatalf("old Agent claimed with another Agent's execution context: %+v", response.Task)
	}
}

func TestClaimTaskChatInputLoadFailureSkipsResumeQueries(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	ctx := context.Background()
	agentID, runtimeID, daemonID := createRuntimeGuardAgent(t, ctx)
	chatSessionID := dbfx.ChatSession(t, agentID, testutil.Cols{
		"title":      "input failure skips resume history",
		"runtime_id": runtimeID,
	})
	taskID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id":      runtimeID,
		"chat_session_id": chatSessionID,
		"priority":        1000,
	})
	dbfx.Exec(t, `UPDATE agent_task_queue SET chat_input_task_id = id WHERE id = $1`, taskID)
	dbfx.Insert(t, "chat_message", testutil.Cols{
		"chat_session_id": chatSessionID,
		"role":            "user",
		"content":         "input query should fail before resume history",
		"task_id":         taskID,
	})

	claimMetrics := obsmetrics.NewBusinessMetrics()
	h := *testHandler
	h.Metrics = claimMetrics
	h.Queries = db.New(&failChatInputQueryDB{delegate: testPool})

	req := newDaemonTokenRequest(http.MethodPost, "/api/daemon/runtimes/"+runtimeID+"/claim", nil,
		testWorkspaceID, daemonID)
	req = testutil.WithURLParams(req, "runtimeId", runtimeID)
	testutil.Call(t, h.ClaimTaskByRuntime, req).Want(http.StatusInternalServerError)

	var status string
	dbfx.QueryRow(t, `SELECT status FROM agent_task_queue WHERE id = $1`, taskID).Scan(&status)
	if status != "dispatched" {
		t.Fatalf("task status after input load failure = %q, want dispatched for redelivery", status)
	}

	registry := prometheus.NewRegistry()
	registry.MustRegister(claimMetrics.Collectors()...)
	families, err := registry.Gather()
	if err != nil {
		t.Fatalf("gather claim metrics: %v", err)
	}
	for _, family := range families {
		switch family.GetName() {
		case "patchbay_chat_claim_session_fallback_needed_total":
			if len(family.Metric) != 1 || family.Metric[0].GetCounter().GetValue() != 0 {
				t.Fatalf("input load failure unexpectedly needed session fallback: %v", family)
			}
		case "patchbay_chat_claim_session_fallback_result_total":
			t.Fatalf("input load failure unexpectedly emitted a session fallback result: %v", family)
		case "patchbay_chat_claim_resume_query_duration_seconds":
			t.Fatalf("input load failure unexpectedly ran a resume-history query: %v", family)
		}
	}
}
