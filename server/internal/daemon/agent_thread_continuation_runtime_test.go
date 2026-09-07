package daemon

import (
	"context"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/patchbay-ai/patchbay/server/internal/daemon/execenv"
	"github.com/patchbay-ai/patchbay/server/pkg/agent"
)

func TestSourceNeutralCodexContinuationUsesPersistentStoreBeforeResumePresenceGate(t *testing.T) {
	t.Parallel()

	task := Task{
		PriorSessionID:        "provider-session",
		PriorWorkDir:          "/already-gc-deleted/root/workdir",
		AgentThreadRootTaskID: "019c6e27-e55b-73d1-87d8-4e01f1f75043",
	}
	ctx := execenv.TaskContextForEnv{
		PriorSessionResumed:   true,
		AgentThreadRootTaskID: task.AgentThreadRootTaskID,
	}
	if !gateResumeToReachableSession(&task, &ctx, "codex", "/fresh/continuation/workdir", true, discardLogger()) {
		t.Fatal("source-neutral Codex continuation was rejected solely because the old workdir was GC'd")
	}
	if task.PriorSessionID == "" || !ctx.PriorSessionResumed {
		t.Fatal("reachable persistent-store resume pointer was cleared")
	}

	codexHome := t.TempDir()
	gated := task
	gatedCtx := ctx
	gateCodexResumeToRolloutPresence(&gated, &gatedCtx, "codex", codexHome, slog.Default())
	if gated.PriorSessionID != "" || gatedCtx.PriorSessionResumed || !gatedCtx.PriorSessionResumeUnavailable || !gated.PriorSessionResumeUnavailable {
		t.Fatalf("missing rollout did not surface the existing unavailable state: task=%+v ctx=%+v", gated, gatedCtx)
	}
	if _, err := os.Stat(codexHome); err != nil {
		t.Fatalf("test codex home disappeared: %v", err)
	}
}

func TestSourceNeutralContinuationFailsBeforeProviderLaunchWhenRolloutMissing(t *testing.T) {
	t.Parallel()

	workspacesRoot := t.TempDir()
	providerMarker := filepath.Join(t.TempDir(), "provider-launched")
	fakeCodex := filepath.Join(t.TempDir(), "codex")
	if err := os.WriteFile(fakeCodex, []byte("#!/bin/sh\nprintf launched > \""+providerMarker+"\"\n"), 0o755); err != nil {
		t.Fatalf("write fake codex: %v", err)
	}

	var startCalls atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.HasSuffix(r.URL.Path, "/start") {
			startCalls.Add(1)
		}
		w.WriteHeader(http.StatusOK)
	}))
	defer srv.Close()

	d := &Daemon{
		client:         NewClient(srv.URL),
		logger:         slog.New(slog.NewTextHandler(io.Discard, nil)),
		workspaces:     make(map[string]*workspaceState),
		runtimeIndex:   map[string]Runtime{"runtime-thread": {ID: "runtime-thread", Provider: "codex"}},
		activeEnvRoots: make(map[string]int),
		activeStores:   make(map[string]int),
		deletingStores: make(map[string]bool),
		cfg: Config{
			WorkspacesRoot: workspacesRoot,
			ServerBaseURL:  srv.URL,
			AgentTimeout:   time.Second,
			Agents:         map[string]AgentEntry{"codex": {Path: fakeCodex}},
		},
	}
	d.activeStoresCond = sync.NewCond(&d.activeStoresMu)
	task := Task{
		ID:                    "task-agent-thread-strict",
		WorkspaceID:           "workspace-thread",
		RuntimeID:             "runtime-thread",
		AgentID:               "agent-thread",
		AuthToken:             "mat-agent-thread",
		AgentThreadMessage:    "continue the previous run",
		AgentThreadRootTaskID: "019c6e27-17e6-7ea0-bcae-d7a0721d775a",
		PriorSessionID:        "missing-rollout-session",
		Agent:                 &AgentData{ID: "agent-thread", Name: "thread-agent"},
	}

	result, err := d.runTask(context.Background(), task, "codex", 0, d.logger)
	if err != nil {
		t.Fatalf("runTask: %v", err)
	}
	if result.Status != "blocked" || !result.SessionRolloutMissing {
		t.Fatalf("result = %+v, want blocked with missing-session state", result)
	}
	if !strings.Contains(result.Comment, "could not be restored") {
		t.Fatalf("result comment = %q, want an unavailable continuation message", result.Comment)
	}
	if got := startCalls.Load(); got != 1 {
		t.Fatalf("StartTask calls = %d, want 1 before terminal failure", got)
	}
	if _, err := os.Stat(providerMarker); !os.IsNotExist(err) {
		t.Fatalf("provider was launched despite missing rollout; marker stat = %v", err)
	}
}

func TestSourceNeutralContinuationDoesNotRetryWithFreshSession(t *testing.T) {
	t.Parallel()

	task := Task{
		AgentThreadMessage:    "continue the previous run",
		AgentThreadRootTaskID: "019c6e27-e55b-73d1-87d8-4e01f1f75043",
	}
	result := agent.Result{Status: "failed", ResumeRejected: true}
	if shouldRetryTaskWithFreshSession(task, result, "provider-session", 0, "codex") {
		t.Fatal("source-neutral continuation must not fall back to a fresh provider session")
	}
}

func TestAntigravityContinuationKeepsSessionWhenPriorWorkdirWasGCd(t *testing.T) {
	t.Parallel()

	task := Task{PriorSessionID: "agy-conversation", PriorWorkDir: "/already-gc-deleted/workdir"}
	ctx := execenv.TaskContextForEnv{PriorSessionResumed: true}
	if !gateResumeToReachableSession(&task, &ctx, "antigravity", "/fresh/continuation/workdir", true, discardLogger()) {
		t.Fatal("Antigravity conversation was rejected solely because the old workdir was GC'd")
	}
	if task.PriorSessionID != "agy-conversation" || !ctx.PriorSessionResumed {
		t.Fatalf("Antigravity resume pointer was cleared: task=%+v ctx=%+v", task, ctx)
	}
}

func TestShouldReusePriorWorkdirForSourceNeutralAgentContinuation(t *testing.T) {
	t.Parallel()

	workspacesRoot := t.TempDir()
	workspaceID := "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
	rootTaskID := "019c6e27-e55b-73d1-87d8-4e01f1f75043"
	childTaskID := "019c7714-3b77-74d1-9866-e1f484aae2ab"
	ctx := execenv.TaskContextForEnv{
		RuntimeID:             "runtime-root",
		AgentID:               "agent-root",
		AgentThreadRootTaskID: rootTaskID,
	}
	env, err := execenv.Prepare(execenv.PrepareParams{
		WorkspacesRoot: workspacesRoot,
		WorkspaceID:    workspaceID,
		TaskID:         rootTaskID,
		Provider:       "claude",
		Task:           ctx,
	}, discardLogger())
	if err != nil {
		t.Fatalf("prepare source-neutral root env: %v", err)
	}
	defer env.Cleanup(true)

	child := Task{
		ID:                    childTaskID,
		AgentID:               ctx.AgentID,
		RuntimeID:             ctx.RuntimeID,
		WorkspaceID:           workspaceID,
		AgentThreadRootTaskID: rootTaskID,
		PriorWorkDir:          env.WorkDir,
	}
	got, ok := shouldReusePriorWorkdir(child, nil, workspacesRoot)
	if !ok || !sameDir(t, got, env.WorkDir) {
		t.Fatalf("source-neutral continuation reuse = (%q, %v), want (%q, true)", got, ok, env.WorkDir)
	}

	wrongRuntime := child
	wrongRuntime.RuntimeID = "runtime-other"
	if _, ok := shouldReusePriorWorkdir(wrongRuntime, nil, workspacesRoot); ok {
		t.Fatal("continuation from another runtime reused the root workdir")
	}
	wrongRoot := child
	wrongRoot.AgentThreadRootTaskID = "019c7714-3b77-74d1-9866-e1f484aae2ac"
	if _, ok := shouldReusePriorWorkdir(wrongRoot, nil, workspacesRoot); ok {
		t.Fatal("continuation from another Agent thread reused the root workdir")
	}
}
