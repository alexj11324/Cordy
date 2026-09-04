package daemon

import (
	"context"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/patchbay-ai/patchbay/server/internal/daemon/execenv"
)

// TestHandleTask_DoesNotCallStartTaskItself is the regression guard for
// issue #3999 race A. handleTask must not call /tasks/{id}/start before
// runner.run — the runner is now responsible for calling StartTask only
// after execenv.Prepare/Reuse has put env.WorkDir on disk, so consumers
// that read status==running can resolve the workdir path without racing
// the daemon's os.MkdirAll.
//
// Before the fix: handleTask called StartTask before invoking the runner,
// flipping the server-side state to "running" while the per-task workdir
// still didn't exist on disk. Hermes/OpenClaw agents that resolved
// /patchbay_workspaces/{ws}/{short-id}/workdir from the running signal
// would then hit FileNotFoundError.
func TestHandleTask_DoesNotCallStartTaskItself(t *testing.T) {
	t.Parallel()

	var (
		startCalls   atomic.Int64
		runnerCalled atomic.Bool
	)

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/provider-authorization"):
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"allowed":true}`))
		case strings.HasSuffix(r.URL.Path, "/start"):
			startCalls.Add(1)
		}
		w.WriteHeader(http.StatusOK)
	}))
	t.Cleanup(srv.Close)

	d := &Daemon{
		client:             NewClient(srv.URL),
		logger:             slog.New(slog.NewTextHandler(io.Discard, nil)),
		workspaces:         make(map[string]*workspaceState),
		runtimeIndex:       map[string]Runtime{"rt-1": {ID: "rt-1", Provider: "claude"}},
		activeEnvRoots:     make(map[string]int),
		cancelPollInterval: time.Hour, // disable poll-cancel path; we only care about the entry-side ordering
	}

	// Fake runner that does NOT call StartTask — production runTask does
	// the call itself, after Prepare/Reuse confirms env.WorkDir on disk.
	d.runner = taskRunnerFunc(func(_ context.Context, _ Task, _ string, _ int, _ *slog.Logger) (TaskResult, error) {
		runnerCalled.Store(true)
		return TaskResult{Status: "completed"}, nil
	})

	task := Task{
		ID:          "task-no-start",
		WorkspaceID: "ws-no-start",
		RuntimeID:   "rt-1",
		IssueID:     "issue-no-start",
		AuthToken:   "mat_task-no-start",
		Agent:       &AgentData{Name: "test-agent"},
	}

	d.handleTask(context.Background(), task, 0)

	if !runnerCalled.Load() {
		t.Fatal("fake runner was never invoked — handleTask aborted before runner.run, can't assert ordering")
	}
	if got := startCalls.Load(); got != 0 {
		t.Fatalf("handleTask called /start %d time(s); StartTask must be runTask's responsibility now (issue #3999 race A)", got)
	}
}

// TestRunTask_StartTaskCalledAfterWorkdirOnDisk is the behavioral regression
// guard for issue #3999 race A. Calls runTask directly with a missing agent
// binary so the run aborts at exec time — but only AFTER reaching the
// post-Prepare StartTask call. The fake server records whether the per-task
// workdir already exists on disk at the moment /start is hit; before the
// fix it did not.
func TestRunTask_StartTaskCalledAfterWorkdirOnDisk(t *testing.T) {
	t.Parallel()

	workspacesRoot := t.TempDir()
	workspaceID := "ws-runtask"
	taskID := "task-runtask-after-mkdir"
	expectedEnvRoot := execenv.PredictRootDir(execenv.RootDirParams{WorkspacesRoot: workspacesRoot, WorkspaceID: workspaceID, TaskID: taskID})
	expectedWorkDir := filepath.Join(expectedEnvRoot, "workdir")

	var (
		startCalled   atomic.Bool
		workdirOnDisk atomic.Bool
		envRootOnDisk atomic.Bool
	)

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.HasSuffix(r.URL.Path, "/start") {
			startCalled.Store(true)
			if info, err := os.Stat(expectedWorkDir); err == nil && info.IsDir() {
				workdirOnDisk.Store(true)
			}
			if info, err := os.Stat(expectedEnvRoot); err == nil && info.IsDir() {
				envRootOnDisk.Store(true)
			}
		}
		w.WriteHeader(http.StatusOK)
	}))
	t.Cleanup(srv.Close)

	// Provider entry intentionally points at a non-existent binary: runTask
	// reaches Prepare → StartTask → ReportProgress before agent.Backend.Run
	// fails at exec time. We don't care about the eventual error; the
	// regression guard is the order of /start vs. os.MkdirAll(envRoot).
	missingBin := filepath.Join(t.TempDir(), "definitely-not-claude")
	d := &Daemon{
		client:         NewClient(srv.URL),
		logger:         slog.New(slog.NewTextHandler(io.Discard, nil)),
		workspaces:     make(map[string]*workspaceState),
		runtimeIndex:   map[string]Runtime{"rt-1": {ID: "rt-1", Provider: "claude"}},
		activeEnvRoots: make(map[string]int),
		cfg: Config{
			WorkspacesRoot: workspacesRoot,
			Agents: map[string]AgentEntry{
				"claude": {Path: missingBin, Model: ""},
			},
		},
	}

	task := Task{
		ID:          taskID,
		WorkspaceID: workspaceID,
		RuntimeID:   "rt-1",
		IssueID:     "issue-runtask",
		AgentID:     "agent-runtask",
		Agent:       &AgentData{ID: "agent-runtask", Name: "test-agent"},
	}

	taskLog := slog.New(slog.NewTextHandler(io.Discard, nil))
	// The Run() failure is expected; we only assert the pre-Run ordering.
	_, _ = d.runTask(context.Background(), task, "claude", 0, taskLog)

	if !startCalled.Load() {
		t.Fatal("runTask did not call /start — Fix A's StartTask placement is missing")
	}
	if !envRootOnDisk.Load() {
		t.Fatal("envRoot did not exist on disk when /start was called — Prepare must run before StartTask (issue #3999 race A)")
	}
	if !workdirOnDisk.Load() {
		t.Fatal("envRoot/workdir did not exist on disk when /start was called — os.MkdirAll must complete before StartTask (issue #3999 race A)")
	}
}

func TestRunTask_InjectsPrivateTaskTempDir(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell-script agent fixture is POSIX-only")
	}

	workspacesRoot := filepath.Join(t.TempDir(), strings.Repeat("long-workspaces-root-", 3))
	workspaceID := "ws-private-temp"
	taskID := "task-private-temp-with-long-id-that-would-overflow-socket-paths"
	envRoot := execenv.PredictRootDir(execenv.RootDirParams{WorkspacesRoot: workspacesRoot, WorkspaceID: workspaceID, TaskID: taskID})

	captureFile := filepath.Join(t.TempDir(), "agent-env.txt")
	fakeBin := filepath.Join(t.TempDir(), "claude")
	script := `#!/bin/sh
if [ -d "$TMPDIR" ]; then
  tmpdir_exists=yes
else
  tmpdir_exists=no
fi
printf 'TMPDIR=%s\nTMP=%s\nTEMP=%s\nTMPDIR_EXISTS=%s\n' "$TMPDIR" "$TMP" "$TEMP" "$tmpdir_exists" > "$CAPTURE_FILE"
IFS= read -r _
printf '%s\n' '{"type":"system","session_id":"sess-private-temp"}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"session_id":"sess-private-temp","result":"done"}'
`
	if err := os.WriteFile(fakeBin, []byte(script), 0o755); err != nil {
		t.Fatalf("write fake agent: %v", err)
	}
	if err := os.Chmod(fakeBin, 0o755); err != nil {
		t.Fatalf("chmod fake agent: %v", err)
	}

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))
	t.Cleanup(srv.Close)

	d := &Daemon{
		client:         NewClient(srv.URL),
		logger:         slog.New(slog.NewTextHandler(io.Discard, nil)),
		workspaces:     make(map[string]*workspaceState),
		runtimeIndex:   map[string]Runtime{"rt-1": {ID: "rt-1", Provider: "claude"}},
		activeEnvRoots: make(map[string]int),
		cfg: Config{
			WorkspacesRoot: workspacesRoot,
			AgentTimeout:   5 * time.Second,
			ServerBaseURL:  srv.URL,
			Agents: map[string]AgentEntry{
				"claude": {Path: fakeBin, Model: ""},
			},
		},
	}

	task := Task{
		ID:          taskID,
		WorkspaceID: workspaceID,
		RuntimeID:   "rt-1",
		IssueID:     "issue-private-temp",
		AgentID:     "agent-private-temp",
		AuthToken:   "mat_private_temp",
		Agent: &AgentData{
			ID:   "agent-private-temp",
			Name: "test-agent",
			CustomEnv: map[string]string{
				"CAPTURE_FILE": captureFile,
				"TMPDIR":       "/shared/tmp",
				"TMP":          "/shared/tmp",
				"TEMP":         "/shared/tmp",
			},
		},
	}

	taskLog := slog.New(slog.NewTextHandler(io.Discard, nil))
	result, err := d.runTask(context.Background(), task, "claude", 0, taskLog)
	if err != nil {
		t.Fatalf("runTask(): %v", err)
	}
	if result.Status != "completed" {
		t.Fatalf("runTask status = %q, want completed (comment=%q)", result.Status, result.Comment)
	}

	raw, err := os.ReadFile(captureFile)
	if err != nil {
		t.Fatalf("read captured agent env: %v", err)
	}
	got := make(map[string]string)
	for _, line := range strings.Split(strings.TrimSpace(string(raw)), "\n") {
		key, value, ok := strings.Cut(line, "=")
		if !ok {
			t.Fatalf("malformed captured env line %q", line)
		}
		got[key] = value
	}
	for _, key := range []string{"TMPDIR", "TMP", "TEMP"} {
		if got[key] == "" {
			t.Fatalf("%s was not captured", key)
		}
		if got[key] != got["TMPDIR"] {
			t.Fatalf("%s = %q, want same private task temp dir %q", key, got[key], got["TMPDIR"])
		}
	}
	if got["TMPDIR_EXISTS"] != "yes" {
		t.Fatalf("fake agent saw TMPDIR_EXISTS=%q, want yes", got["TMPDIR_EXISTS"])
	}
	taskTempDir := got["TMPDIR"]
	if strings.HasPrefix(taskTempDir, envRoot) {
		t.Fatalf("task temp dir %q must not live under long env root %q", taskTempDir, envRoot)
	}
	if len(taskTempDir) > 80 {
		t.Fatalf("task temp dir %q length = %d, want <= 80 for Unix-socket headroom", taskTempDir, len(taskTempDir))
	}
	if _, err := os.Stat(taskTempDir); !os.IsNotExist(err) {
		t.Fatalf("expected task temp dir %q to be cleaned after run, stat err=%v", taskTempDir, err)
	}
}

// TestTaskTempBaseDir covers the PATCHBAY_AGENT_TEMP_BASE validation contract:
// Windows ignores it, while Unix honors a valid absolute directory and reports
// unusable configured bases from the real task-directory creation instead of
// silently falling back to /tmp.
func TestTaskTempBaseDir(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Setenv("PATCHBAY_AGENT_TEMP_BASE", `C:\configured-but-ignored`)
		got, configured, err := taskTempBaseDir()
		if err != nil {
			t.Fatalf("taskTempBaseDir(): %v", err)
		}
		if configured {
			t.Fatal("taskTempBaseDir() marked Windows override as configured")
		}
		if got != socketSafeTempBaseDir() {
			t.Fatalf("taskTempBaseDir() = %q, want platform default %q", got, socketSafeTempBaseDir())
		}
		return
	}

	validBase := t.TempDir()
	cases := []struct {
		name           string
		value          string
		set            bool
		want           string
		wantConfigured bool
		wantErr        bool
	}{
		{name: "unset keeps platform default", set: false, want: socketSafeTempBaseDir()},
		{name: "empty keeps platform default", set: true, value: "  ", want: socketSafeTempBaseDir()},
		{name: "valid absolute dir is honored", set: true, value: validBase, want: validBase, wantConfigured: true},
		{name: "relative path rejected", set: true, value: "relative/base", wantConfigured: true, wantErr: true},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			// Register the restore hook in both branches: t.Setenv remembers
			// whether the variable was originally set and undoes either case.
			t.Setenv("PATCHBAY_AGENT_TEMP_BASE", tc.value)
			if !tc.set {
				if err := os.Unsetenv("PATCHBAY_AGENT_TEMP_BASE"); err != nil {
					t.Fatalf("unset PATCHBAY_AGENT_TEMP_BASE: %v", err)
				}
			}
			got, configured, err := taskTempBaseDir()
			if configured != tc.wantConfigured {
				t.Fatalf("taskTempBaseDir() configured = %v, want %v", configured, tc.wantConfigured)
			}
			if tc.wantErr {
				if err == nil {
					t.Fatalf("taskTempBaseDir() = %q, want error", got)
				}
				// The message must name the variable the operator set, so the
				// failure is actionable rather than a bare mkdir/stat error.
				if !strings.Contains(err.Error(), "PATCHBAY_AGENT_TEMP_BASE") {
					t.Fatalf("error %q does not mention PATCHBAY_AGENT_TEMP_BASE", err)
				}
				return
			}
			if err != nil {
				t.Fatalf("taskTempBaseDir(): %v", err)
			}
			if got != tc.want {
				t.Fatalf("taskTempBaseDir() = %q, want %q", got, tc.want)
			}
		})
	}

	t.Run("configured base creates private 0700 task dir", func(t *testing.T) {
		t.Setenv("PATCHBAY_AGENT_TEMP_BASE", validBase)
		dir, lock, err := ensureTaskTempDir("root", "ws", "task")
		if err != nil {
			t.Fatalf("ensureTaskTempDir(): %v", err)
		}
		t.Cleanup(func() {
			execenv.ReleaseTaskTempLock(lock)
			_ = os.RemoveAll(dir)
		})
		info, err := os.Stat(dir)
		if err != nil {
			t.Fatalf("stat task temp dir: %v", err)
		}
		if info.Mode().Perm() != 0o700 {
			t.Fatalf("task temp dir mode = %o, want 0700", info.Mode().Perm())
		}
	})

	notDir := filepath.Join(t.TempDir(), "file")
	if err := os.WriteFile(notDir, []byte("x"), 0o600); err != nil {
		t.Fatalf("write notDir fixture: %v", err)
	}
	readOnlyBase := filepath.Join(t.TempDir(), "read-only")
	if err := os.Mkdir(readOnlyBase, 0o500); err != nil {
		t.Fatalf("mkdir readOnlyBase fixture: %v", err)
	}
	// t.TempDir cleanup needs to descend into it again.
	t.Cleanup(func() { _ = os.Chmod(readOnlyBase, 0o700) })

	for _, tc := range []struct {
		name string
		base string
	}{
		{name: "missing dir rejected", base: filepath.Join(validBase, "missing")},
		{name: "non-directory rejected", base: notDir},
		{name: "non-writable dir rejected", base: readOnlyBase},
	} {
		t.Run(tc.name, func(t *testing.T) {
			t.Setenv("PATCHBAY_AGENT_TEMP_BASE", tc.base)
			dir, lock, err := ensureTaskTempDir("root", "ws", "task")
			if err == nil {
				execenv.ReleaseTaskTempLock(lock)
				_ = os.RemoveAll(dir)
				if tc.base == readOnlyBase {
					t.Skip("process can write to the read-only fixture")
				}
				t.Fatalf("ensureTaskTempDir() = %q with unusable PATCHBAY_AGENT_TEMP_BASE, want error", dir)
			}
			if !strings.Contains(err.Error(), "PATCHBAY_AGENT_TEMP_BASE") {
				t.Fatalf("error %q does not mention PATCHBAY_AGENT_TEMP_BASE", err)
			}
		})
	}
}

// TestRunTask_TaskTempBaseOverride is the PATCHBAY_AGENT_TEMP_BASE counterpart
// of TestRunTask_InjectsPrivateTaskTempDir: with the variable set, all three
// temp vars point at one fresh private dir under the configured base, agent
// custom_env still cannot override them, and the dir is removed on task exit.
func TestRunTask_TaskTempBaseOverride(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell-script agent fixture is POSIX-only")
	}

	tempBase := t.TempDir()
	t.Setenv("PATCHBAY_AGENT_TEMP_BASE", tempBase)

	workspacesRoot := t.TempDir()
	workspaceID := "ws-temp-base"
	taskID := "task-temp-base"

	captureFile := filepath.Join(t.TempDir(), "agent-env.txt")
	fakeBin := filepath.Join(t.TempDir(), "claude")
	script := `#!/bin/sh
printf 'TMPDIR=%s\nTMP=%s\nTEMP=%s\n' "$TMPDIR" "$TMP" "$TEMP" > "$CAPTURE_FILE"
IFS= read -r _
printf '%s\n' '{"type":"system","session_id":"sess-temp-base"}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"session_id":"sess-temp-base","result":"done"}'
`
	if err := os.WriteFile(fakeBin, []byte(script), 0o755); err != nil {
		t.Fatalf("write fake agent: %v", err)
	}

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))
	t.Cleanup(srv.Close)

	d := &Daemon{
		client:         NewClient(srv.URL),
		logger:         slog.New(slog.NewTextHandler(io.Discard, nil)),
		workspaces:     make(map[string]*workspaceState),
		runtimeIndex:   map[string]Runtime{"rt-1": {ID: "rt-1", Provider: "claude"}},
		activeEnvRoots: make(map[string]int),
		cfg: Config{
			WorkspacesRoot: workspacesRoot,
			AgentTimeout:   5 * time.Second,
			ServerBaseURL:  srv.URL,
			Agents: map[string]AgentEntry{
				"claude": {Path: fakeBin, Model: ""},
			},
		},
	}

	task := Task{
		ID:          taskID,
		WorkspaceID: workspaceID,
		RuntimeID:   "rt-1",
		IssueID:     "issue-temp-base",
		AgentID:     "agent-temp-base",
		AuthToken:   "mat_temp_base",
		Agent: &AgentData{
			ID:   "agent-temp-base",
			Name: "test-agent",
			CustomEnv: map[string]string{
				"CAPTURE_FILE": captureFile,
				"TMPDIR":       "/shared/tmp",
				"TMP":          "/shared/tmp",
				"TEMP":         "/shared/tmp",
			},
		},
	}

	taskLog := slog.New(slog.NewTextHandler(io.Discard, nil))
	result, err := d.runTask(context.Background(), task, "claude", 0, taskLog)
	if err != nil {
		t.Fatalf("runTask(): %v", err)
	}
	if result.Status != "completed" {
		t.Fatalf("runTask status = %q, want completed (comment=%q)", result.Status, result.Comment)
	}

	raw, err := os.ReadFile(captureFile)
	if err != nil {
		t.Fatalf("read captured agent env: %v", err)
	}
	got := make(map[string]string)
	for _, line := range strings.Split(strings.TrimSpace(string(raw)), "\n") {
		key, value, ok := strings.Cut(line, "=")
		if !ok {
			t.Fatalf("malformed captured env line %q", line)
		}
		got[key] = value
	}
	taskTempDir := got["TMPDIR"]
	if taskTempDir == "" {
		t.Fatal("TMPDIR was not captured")
	}
	for _, key := range []string{"TMP", "TEMP"} {
		if got[key] != taskTempDir {
			t.Fatalf("%s = %q, want same private task temp dir %q", key, got[key], taskTempDir)
		}
	}
	if filepath.Dir(taskTempDir) != tempBase {
		t.Fatalf("task temp dir %q is not directly under configured base %q", taskTempDir, tempBase)
	}
	if _, err := os.Stat(taskTempDir); !os.IsNotExist(err) {
		t.Fatalf("expected task temp dir %q to be cleaned after run, stat err=%v", taskTempDir, err)
	}
}

// TestRunTask_TaskTempBaseInvalidFailsStartup pins the "no silent fallback"
// half of the contract at the level operators experience it: an unusable
// PATCHBAY_AGENT_TEMP_BASE fails the task with a message naming the variable,
// and the agent never starts against a /tmp dir it did not ask for.
func TestRunTask_TaskTempBaseInvalidFailsStartup(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell-script agent fixture is POSIX-only")
	}

	missingBase := filepath.Join(t.TempDir(), "does-not-exist")
	t.Setenv("PATCHBAY_AGENT_TEMP_BASE", missingBase)

	workspacesRoot := t.TempDir()
	captureFile := filepath.Join(t.TempDir(), "agent-env.txt")
	fakeBin := filepath.Join(t.TempDir(), "claude")
	script := `#!/bin/sh
printf 'ran\n' > "$CAPTURE_FILE"
`
	if err := os.WriteFile(fakeBin, []byte(script), 0o755); err != nil {
		t.Fatalf("write fake agent: %v", err)
	}

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))
	t.Cleanup(srv.Close)

	d := &Daemon{
		client:         NewClient(srv.URL),
		logger:         slog.New(slog.NewTextHandler(io.Discard, nil)),
		workspaces:     make(map[string]*workspaceState),
		runtimeIndex:   map[string]Runtime{"rt-1": {ID: "rt-1", Provider: "claude"}},
		activeEnvRoots: make(map[string]int),
		cfg: Config{
			WorkspacesRoot: workspacesRoot,
			AgentTimeout:   5 * time.Second,
			ServerBaseURL:  srv.URL,
			Agents: map[string]AgentEntry{
				"claude": {Path: fakeBin, Model: ""},
			},
		},
	}

	task := Task{
		ID:          "task-temp-base-invalid",
		WorkspaceID: "ws-temp-base-invalid",
		RuntimeID:   "rt-1",
		IssueID:     "issue-temp-base-invalid",
		AgentID:     "agent-temp-base-invalid",
		AuthToken:   "mat_temp_base_invalid",
		Agent: &AgentData{
			ID:        "agent-temp-base-invalid",
			Name:      "test-agent",
			CustomEnv: map[string]string{"CAPTURE_FILE": captureFile},
		},
	}

	taskLog := slog.New(slog.NewTextHandler(io.Discard, nil))
	_, err := d.runTask(context.Background(), task, "claude", 0, taskLog)
	if err == nil {
		t.Fatal("runTask() succeeded with an unusable PATCHBAY_AGENT_TEMP_BASE, want failure")
	}
	if !strings.Contains(err.Error(), "PATCHBAY_AGENT_TEMP_BASE") {
		t.Fatalf("runTask() error = %v, want it to name PATCHBAY_AGENT_TEMP_BASE", err)
	}
	if _, statErr := os.Stat(captureFile); !os.IsNotExist(statErr) {
		t.Fatalf("agent ran despite the temp-base failure, stat err=%v", statErr)
	}
}

func TestRunTask_ExtendsPrepareLeaseDuringStartTask(t *testing.T) {
	oldRefresh := taskPrepareLeaseRefresh
	oldTimeout := taskPrepareLeaseTimeout
	taskPrepareLeaseRefresh = 10 * time.Millisecond
	taskPrepareLeaseTimeout = 500 * time.Millisecond
	t.Cleanup(func() {
		taskPrepareLeaseRefresh = oldRefresh
		taskPrepareLeaseTimeout = oldTimeout
	})

	workspacesRoot := t.TempDir()
	workspaceID := "ws-runtask-start-lease"
	taskID := "task-runtask-start-lease"
	var (
		startEntered     atomic.Bool
		leaseDuringStart atomic.Bool
		closeLeaseOnce   sync.Once
	)
	leaseSeenDuringStart := make(chan struct{})

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/prepare-lease"):
			if startEntered.Load() {
				leaseDuringStart.Store(true)
				closeLeaseOnce.Do(func() { close(leaseSeenDuringStart) })
			}
			w.WriteHeader(http.StatusOK)
		case strings.HasSuffix(r.URL.Path, "/start"):
			startEntered.Store(true)
			select {
			case <-leaseSeenDuringStart:
			case <-time.After(2 * time.Second):
			}
			w.WriteHeader(http.StatusOK)
		default:
			w.WriteHeader(http.StatusOK)
		}
	}))
	t.Cleanup(srv.Close)

	missingBin := filepath.Join(t.TempDir(), "definitely-not-claude")
	d := &Daemon{
		client:         NewClient(srv.URL),
		logger:         slog.New(slog.NewTextHandler(io.Discard, nil)),
		workspaces:     make(map[string]*workspaceState),
		runtimeIndex:   map[string]Runtime{"rt-1": {ID: "rt-1", Provider: "claude"}},
		activeEnvRoots: make(map[string]int),
		cfg: Config{
			WorkspacesRoot: workspacesRoot,
			Agents: map[string]AgentEntry{
				"claude": {Path: missingBin, Model: ""},
			},
		},
	}

	task := Task{
		ID:          taskID,
		WorkspaceID: workspaceID,
		RuntimeID:   "rt-1",
		IssueID:     "issue-runtask-start-lease",
		AgentID:     "agent-runtask-start-lease",
		Agent:       &AgentData{ID: "agent-runtask-start-lease", Name: "test-agent"},
	}

	taskLog := slog.New(slog.NewTextHandler(io.Discard, nil))
	_, _ = d.runTask(context.Background(), task, "claude", 0, taskLog)

	if !startEntered.Load() {
		t.Fatal("runTask did not call /start")
	}
	if !leaseDuringStart.Load() {
		t.Fatal("prepare lease was not extended while /start was still in flight")
	}
}

type blockedPrepareLeaseTransport struct {
	base         http.RoundTripper
	startEntered <-chan struct{}
	cancel       context.CancelCauseFunc
	started      chan struct{}
	stopped      chan error
	startedOnce  sync.Once
	stoppedOnce  sync.Once
}

func (t *blockedPrepareLeaseTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	if strings.HasSuffix(req.URL.Path, "/prepare-lease") {
		select {
		case <-t.startEntered:
		default:
			return &http.Response{StatusCode: http.StatusOK, Body: io.NopCloser(strings.NewReader(""))}, nil
		}
		t.startedOnce.Do(func() {
			close(t.started)
			t.cancel(errTaskPrepareTimeout)
		})
		<-req.Context().Done()
		t.stoppedOnce.Do(func() { t.stopped <- context.Cause(req.Context()) })
		return nil, req.Context().Err()
	}
	return t.base.RoundTrip(req)
}

func TestRunTask_PrepareTimeoutStopsLeaseDuringBlockedStartTask(t *testing.T) {
	oldRefresh := taskPrepareLeaseRefresh
	oldTimeout := taskPrepareLeaseTimeout
	taskPrepareLeaseRefresh = 10 * time.Millisecond
	taskPrepareLeaseTimeout = 500 * time.Millisecond
	t.Cleanup(func() {
		taskPrepareLeaseRefresh = oldRefresh
		taskPrepareLeaseTimeout = oldTimeout
	})

	startEntered := make(chan struct{})
	var closeStartOnce sync.Once
	releaseStart := make(chan struct{})
	var releaseStartOnce sync.Once
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/prepare-lease"):
			w.WriteHeader(http.StatusOK)
		case strings.HasSuffix(r.URL.Path, "/start"):
			closeStartOnce.Do(func() { close(startEntered) })
			<-releaseStart
			w.WriteHeader(http.StatusOK)
		default:
			w.WriteHeader(http.StatusOK)
		}
	}))
	t.Cleanup(func() {
		releaseStartOnce.Do(func() { close(releaseStart) })
		srv.Close()
	})

	client := NewClient(srv.URL)

	workspacesRoot := t.TempDir()
	fakeBin := filepath.Join(t.TempDir(), "claude")
	if err := os.WriteFile(fakeBin, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatalf("write fake agent: %v", err)
	}
	d := &Daemon{
		client:             client,
		logger:             slog.New(slog.NewTextHandler(io.Discard, nil)),
		workspaces:         make(map[string]*workspaceState),
		runtimeIndex:       map[string]Runtime{"rt-1": {ID: "rt-1", Provider: "claude"}},
		activeEnvRoots:     make(map[string]int),
		taskPrepareTimeout: time.Nanosecond,
		cfg: Config{
			WorkspacesRoot: workspacesRoot,
			Agents: map[string]AgentEntry{
				"claude": {Path: fakeBin},
			},
		},
	}

	task := Task{
		ID:          "task-runtask-start-timeout",
		WorkspaceID: "ws-runtask-start-timeout",
		RuntimeID:   "rt-1",
		IssueID:     "issue-runtask-start-timeout",
		AgentID:     "agent-runtask-start-timeout",
		Agent:       &AgentData{ID: "agent-runtask-start-timeout", Name: "test-agent"},
	}
	taskLog := slog.New(slog.NewTextHandler(io.Discard, nil))
	// Exercise the real deadline independently of filesystem preparation speed.
	// A 150ms deadline used to race that work and could expire before /start,
	// even though runTask correctly cancelled the preparation.
	_, err := d.runTask(context.Background(), task, "claude", 0, taskLog)
	if !errors.Is(err, errTaskPrepareTimeout) {
		t.Fatalf("runTask error = %v, want task prepare timeout", err)
	}
	select {
	case <-startEntered:
		t.Fatal("expired preparation reached /start")
	default:
	}

	// Deliver the same timeout cause only after both requests are in flight.
	// This phase proves cancellation and joining, not the timer (covered above).
	d.taskPrepareTimeout = 0
	ctx, cancel := context.WithCancelCause(context.Background())
	transport := &blockedPrepareLeaseTransport{
		base:         client.client.Transport,
		startEntered: startEntered,
		cancel:       cancel,
		started:      make(chan struct{}),
		stopped:      make(chan error, 1),
	}
	client.client.Transport = transport
	runDone := make(chan struct{})
	var runErr error
	go func() {
		_, runErr = d.runTask(ctx, task, "claude", 0, taskLog)
		close(runDone)
	}()
	t.Cleanup(func() {
		cancel(nil)
		releaseStartOnce.Do(func() { close(releaseStart) })
		select {
		case <-runDone:
		case <-time.After(5 * time.Second):
			t.Error("runTask did not stop during cleanup")
		}
	})
	guard := time.NewTimer(5 * time.Second)
	defer guard.Stop()
	select {
	case <-runDone:
	case <-guard.C:
		t.Fatal("prepare timeout did not stop blocked /start")
	}
	if !errors.Is(runErr, errTaskPrepareTimeout) {
		t.Fatalf("runTask error = %v, want task prepare timeout", runErr)
	}
	for _, checkpoint := range []<-chan struct{}{startEntered, transport.started} {
		select {
		case <-checkpoint:
		default:
			t.Fatal("timeout was not delivered while both requests were in flight")
		}
	}
	select {
	case cause := <-transport.stopped:
		if !errors.Is(cause, errTaskPrepareTimeout) {
			t.Fatalf("prepare lease stopped with %v, want task prepare timeout", cause)
		}
	default:
		t.Fatal("runTask returned before cancelling the in-flight prepare lease")
	}
	if got := taskRunFailureReason(runErr); got != "timeout" {
		t.Fatalf("taskRunFailureReason = %q, want retryable platform timeout", got)
	}
}

// TestHandleTask_KeepsEnvRootActiveAcrossCompletion is the regression guard
// for issue #3999 race B. After runner.run returns, the in-process active
// guard installed inside runTask (defer unmarkActiveEnvRoot at the
// goroutine's exit) has already fired by the time handleTask calls
// reportTaskResult and execenv.WriteGCMeta. Without an outer guard at the
// handleTask level, the GC loop sees a window where the directory has
// neither isActiveEnvRoot nor a .gc_meta.json file — falling through to
// orphanByMTime, gated only by the 72h GCOrphanTTL.
//
// This test fakes the inner guard's lifecycle (mark + deferred unmark),
// then asserts that at the moment /complete is hit (i.e. between runner.run
// returning and WriteGCMeta running), isActiveEnvRoot(envRoot) is still
// true thanks to the outer guard handleTask installs.
func TestHandleTask_KeepsEnvRootActiveAcrossCompletion(t *testing.T) {
	t.Parallel()

	workspacesRoot := t.TempDir()
	workspaceID := "ws-active-during-complete"
	taskID := "task-active-during-complete"
	expectedEnvRoot := execenv.PredictRootDir(execenv.RootDirParams{WorkspacesRoot: workspacesRoot, WorkspaceID: workspaceID, TaskID: taskID})

	var (
		completeCalled   atomic.Bool
		activeAtComplete atomic.Bool
	)

	d := &Daemon{
		logger:             slog.New(slog.NewTextHandler(io.Discard, nil)),
		workspaces:         make(map[string]*workspaceState),
		runtimeIndex:       map[string]Runtime{"rt-1": {ID: "rt-1", Provider: "claude"}},
		activeEnvRoots:     make(map[string]int),
		cancelPollInterval: time.Hour,
		cfg:                Config{WorkspacesRoot: workspacesRoot},
	}

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.HasSuffix(r.URL.Path, "/provider-authorization") {
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"allowed":true}`))
			return
		}
		if strings.HasSuffix(r.URL.Path, "/complete") {
			completeCalled.Store(true)
			// This is the exact window race B exposed: the inner deferred
			// unmark has already fired (see fake runner below); only the
			// outer guard installed by handleTask keeps the env root in the
			// active set at this moment.
			if d.isActiveEnvRoot(expectedEnvRoot) {
				activeAtComplete.Store(true)
			}
		}
		w.WriteHeader(http.StatusOK)
	}))
	t.Cleanup(srv.Close)
	d.client = NewClient(srv.URL)

	// Fake runner mimics the real runTask's mark/defer-unmark pair. Without
	// the outer guard added in handleTask, the deferred unmark would bring
	// isActiveEnvRoot back to false before reportTaskResult fires.
	d.runner = taskRunnerFunc(func(_ context.Context, tk Task, _ string, _ int, _ *slog.Logger) (TaskResult, error) {
		predicted := execenv.PredictRootDir(taskRootDirParams(d.cfg.WorkspacesRoot, tk))
		d.markActiveEnvRoot(predicted)
		defer d.unmarkActiveEnvRoot(predicted)
		return TaskResult{
			Status:  "completed",
			EnvRoot: predicted,
		}, nil
	})

	task := Task{
		ID:          taskID,
		WorkspaceID: workspaceID,
		RuntimeID:   "rt-1",
		IssueID:     "issue-active-during-complete",
		AuthToken:   "mat_task-active-during-complete",
		Agent:       &AgentData{Name: "test-agent"},
	}

	d.handleTask(context.Background(), task, 0)

	if !completeCalled.Load() {
		t.Fatal("/complete was never hit — handleTask did not reach reportTaskResult")
	}
	if !activeAtComplete.Load() {
		t.Fatal("env root was NOT in the active set at /complete time — issue #3999 race B regression: GC could reclaim the directory between runner.run returning and WriteGCMeta landing on disk")
	}
	// And the outer guard must have been released by the time handleTask
	// returned, otherwise we'd be leaking active marks across tasks.
	if d.isActiveEnvRoot(expectedEnvRoot) {
		t.Fatal("env root remained active after handleTask returned — outer guard's deferred unmark did not fire")
	}
}
