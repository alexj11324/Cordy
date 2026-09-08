package daemon

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/orvilo-ai/orvilo/server/internal/daemon/terminalreport"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

func TestPreflightAuthProtectsPendingTerminalReportDuringOutage(t *testing.T) {
	t.Cleanup(stubAgentVersion(t))
	savedID, orphanID, runtimeID, workspaceID := uuid.NewString(), uuid.NewString(), uuid.NewString(), uuid.NewString()
	const fence = "57123713200000001"
	var mu sync.Mutex
	states := map[string]string{savedID: "running", orphanID: "running"}
	reruns := map[string]int{}
	reportAttempts, recoveryCalls := 0, 0
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		defer mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		switch r.URL.Path {
		case "/api/tokens/current/renew":
			_, _ = w.Write([]byte(`{"expires_at":"2099-01-01T00:00:00Z","renewed":false}`))
		case "/api/daemon/workspaces":
			_ = json.NewEncoder(w).Encode([]WorkspaceInfo{{ID: workspaceID, Name: "outbox restart"}})
		case "/api/daemon/workspaces/" + workspaceID + "/runtime-profiles":
			_ = json.NewEncoder(w).Encode(RuntimeProfilesResponse{WorkspaceID: workspaceID})
		case "/api/daemon/register":
			_ = json.NewEncoder(w).Encode(RegisterResponse{Runtimes: []Runtime{{ID: runtimeID, Provider: "claude", Status: "online"}}, Repos: []RepoData{}})
		case "/api/daemon/runtimes/" + runtimeID + "/recover-orphans/preserve-results":
			recoveryCalls++
			var body protocol.RecoverOrphansRequest
			if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
				t.Error(err)
			}
			preserved := false
			for _, pending := range body.PendingTerminalReports {
				if pending.TaskID == savedID && pending.ClaimFence == fence {
					preserved = true
				}
			}
			for id, state := range states {
				if state == "running" && !(id == savedID && preserved) {
					states[id] = "failed"
					reruns[id]++
				}
			}
			_, _ = w.Write([]byte(`{"orphaned":1,"retried":1}`))
		case "/api/daemon/tasks/" + savedID + "/complete":
			reportAttempts++
			w.WriteHeader(http.StatusServiceUnavailable)
			_, _ = w.Write([]byte(`{"error":"terminal callback temporarily unavailable"}`))
		default:
			http.NotFound(w, r)
		}
	}))
	t.Cleanup(srv.Close)
	root := filepath.Join(t.TempDir(), "terminal-reports")
	store, err := terminalreport.Open(root)
	if err != nil {
		t.Fatal(err)
	}
	report, err := terminalreport.NewReport(savedID, fence, "complete", []byte(`{"output":"already implemented"}`), time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if _, err := store.Save(report); err != nil {
		t.Fatal(err)
	}
	for restart := 0; restart < 2; restart++ {
		d := freshDaemon(srv.URL)
		d.cfg.Agents = map[string]AgentEntry{"claude": {Path: filepath.Join(t.TempDir(), "test-only-missing-cli")}}
		d.terminalStore, err = terminalreport.Open(root)
		if err != nil {
			t.Fatal(err)
		}
		d.terminalSender = terminalreport.NewSender(d.terminalStore, d.client, d.logger)
		if err := d.preflightAuth(context.Background()); err != nil {
			t.Fatal(err)
		}
		if err := d.terminalSender.Flush(context.Background()); err != nil {
			t.Fatal(err)
		}
		pending, err := d.terminalStore.Pending()
		if err != nil || len(pending) != 1 {
			t.Fatalf("restart %d pending=%v err=%v", restart, pending, err)
		}
	}
	mu.Lock()
	defer mu.Unlock()
	if recoveryCalls != 2 || reportAttempts != 2 {
		t.Fatalf("startup/report calls=%d/%d", recoveryCalls, reportAttempts)
	}
	if states[savedID] != "running" || reruns[savedID] != 0 {
		t.Fatalf("saved result treated as orphan: status=%s reruns=%d", states[savedID], reruns[savedID])
	}
	if states[orphanID] != "failed" || reruns[orphanID] != 1 {
		t.Fatalf("unrelated orphan was not recovered once: status=%s reruns=%d", states[orphanID], reruns[orphanID])
	}
}

func TestRecoverOrphansDoesNotFallBackWhenOlderServerIgnoresProtection(t *testing.T) {
	var destructiveCalls atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/daemon/runtimes/runtime/recover-orphans" {
			destructiveCalls.Add(1)
			w.WriteHeader(http.StatusOK)
			return
		}
		http.NotFound(w, r)
	}))
	t.Cleanup(srv.Close)
	d := testReportingDaemon(t, srv.URL)
	report, err := terminalreport.NewReport(uuid.NewString(), "57123713200000001", "complete", []byte(`{"output":"saved success"}`), time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if _, err := d.terminalStore.Save(report); err != nil {
		t.Fatal(err)
	}
	if err := d.recoverOrphans(context.Background(), "runtime"); err == nil {
		t.Fatal("older server unexpectedly accepted protected recovery")
	}
	if destructiveCalls.Load() != 0 {
		t.Fatal("pending result fell back to destructive legacy recovery")
	}
	pending, err := d.terminalStore.Pending()
	if err != nil || len(pending) != 1 {
		t.Fatalf("pending result not retained: %+v, %v", pending, err)
	}
}

func TestRecoverOrphansDoesNotDiscardResultsWhenOutboxCannotBeRead(t *testing.T) {
	var calls atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { calls.Add(1); w.WriteHeader(http.StatusOK) }))
	t.Cleanup(srv.Close)
	d := testReportingDaemon(t, srv.URL)
	// Replace the readable root after opening, reproducing a disk/read failure.
	root := filepath.Join(t.TempDir(), "outbox")
	store, err := terminalreport.Open(root)
	if err != nil {
		t.Fatal(err)
	}
	d.terminalStore = store
	if err := os.Rename(root, root+".unavailable"); err != nil {
		t.Fatal(err)
	}
	if err := d.recoverOrphans(context.Background(), uuid.NewString()); err == nil {
		t.Fatal("unreadable outbox allowed orphan recovery")
	}
	if calls.Load() != 0 {
		t.Fatalf("unreadable outbox sent %d destructive recovery requests", calls.Load())
	}
}

func TestOrphanRecoveryFailurePausesClaimsUntilAFullRetrySucceeds(t *testing.T) {
	var recoveryCalls atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/daemon/runtimes/runtime/recover-orphans" {
			recoveryCalls.Add(1)
			w.WriteHeader(http.StatusOK)
			return
		}
		http.NotFound(w, r)
	}))
	t.Cleanup(srv.Close)
	d := testReportingDaemon(t, srv.URL)
	d.workspaces = map[string]*workspaceState{
		"workspace": {runtimeIDs: []string{"runtime"}},
	}
	root := filepath.Join(t.TempDir(), "outbox")
	store, err := terminalreport.Open(root)
	if err != nil {
		t.Fatal(err)
	}
	d.terminalStore = store
	if err := os.Rename(root, root+".unavailable"); err != nil {
		t.Fatal(err)
	}
	if err := d.recoverOrphans(context.Background(), "runtime"); err == nil {
		t.Fatal("unreadable terminal outbox allowed orphan recovery")
	}
	if !d.terminalRecoveryFailed.Load() || d.tryEnterClaim() {
		t.Fatal("orphan recovery failure did not pause claims")
	}
	if err := os.Rename(root+".unavailable", root); err != nil {
		t.Fatal(err)
	}
	if err := d.recoverTrackedOrphans(context.Background()); err != nil {
		t.Fatal(err)
	}
	if d.terminalRecoveryFailed.Load() || !d.tryEnterClaim() {
		t.Fatal("successful full recovery did not release the claim barrier")
	}
	d.exitClaim()
	if recoveryCalls.Load() != 1 {
		t.Fatalf("recovery calls = %d, want one successful retry", recoveryCalls.Load())
	}
}
