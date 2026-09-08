package daemon

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"sync/atomic"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/orvilo-ai/orvilo/server/internal/daemon/terminalreport"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

func testReportingDaemon(t *testing.T, serverURL string) *Daemon {
	t.Helper()
	store, err := terminalreport.Open(filepath.Join(t.TempDir(), "results"))
	if err != nil {
		t.Fatal(err)
	}
	logger := slog.New(slog.NewTextHandler(io.Discard, nil))
	d := &Daemon{client: NewClient(serverURL), logger: logger, terminalStore: store}
	d.terminalSender = terminalreport.NewSender(store, d.client, logger)
	return d
}

func TestFencedResultPersistsBeforeAnyRequestAndSurvivesCancellation(t *testing.T) {
	var calls atomic.Int32
	task := Task{ID: uuid.NewString(), ClaimFence: "57123713200000001"}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		if r.URL.Path != "/api/daemon/tasks/"+task.ID+"/complete" {
			t.Errorf("unexpected report route %s", r.URL.Path)
		}
		data, err := io.ReadAll(r.Body)
		if err != nil {
			t.Error(err)
			return
		}
		var body struct {
			Identity protocol.TerminalReportIdentity `json:"terminal_report"`
			Output   string                          `json:"output"`
			Branch   string                          `json:"branch_name"`
			Retired  string                          `json:"retired_session_id"`
		}
		if err := json.Unmarshal(data, &body); err != nil {
			t.Error(err)
			return
		}
		digest, err := protocol.TerminalReportDigest("complete", data)
		if err != nil || digest != body.Identity.PayloadSHA256 || body.Identity.ClaimFence != task.ClaimFence {
			t.Errorf("invalid report identity/digest: %+v, %v", body.Identity, err)
		}
		if body.Output != "work\x00finished" || body.Branch != "agent/result" || body.Retired != "retired-session" {
			t.Errorf("lost execution evidence: %+v", body)
		}
		json.NewEncoder(w).Encode(protocol.TerminalReportAck{TerminalReportIdentity: body.Identity, TaskID: task.ID, Status: "accepted", TaskStatus: "completed"})
	}))
	t.Cleanup(srv.Close)
	d := testReportingDaemon(t, srv.URL)
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	d.reportTaskResult(ctx, task, TaskResult{Status: "completed", Comment: "work\x00finished", BranchName: "agent/result", RetiredSessionID: "retired-session"}, d.logger)
	if calls.Load() != 0 {
		t.Fatal("result reporting made a network call before sender ran")
	}
	reports, err := d.terminalStore.Pending()
	if err != nil || len(reports) != 1 {
		t.Fatalf("cancelled execution did not persist report: %+v, %v", reports, err)
	}
	if err := d.terminalSender.Flush(context.Background()); err != nil {
		t.Fatal(err)
	}
	if calls.Load() != 1 {
		t.Fatalf("wanted one callback, got %d", calls.Load())
	}
	if pending, _ := d.terminalSender.Stats(); pending != 0 {
		t.Fatal("valid acknowledgement did not clear report")
	}
}

func TestFencedPersistenceFailureStopsClaimsWithoutFailureCallback(t *testing.T) {
	var calls atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { calls.Add(1) }))
	t.Cleanup(srv.Close)
	d := testReportingDaemon(t, srv.URL)
	// An unavailable store is a local persistence failure, never a reason to
	// turn a successfully executed result into a failure callback.
	d.terminalStore = nil
	d.reportTaskResult(context.Background(), Task{ID: uuid.NewString(), ClaimFence: "57123713200000001"}, TaskResult{Status: "completed", Comment: "finished"}, d.logger)
	if calls.Load() != 0 || !d.terminalPersistenceFailed.Load() || d.tryEnterClaim() || d.trySetClaimBarrier() {
		t.Fatal("persistence failure allowed another claim, automatic restart or network fallback")
	}
}

func TestTerminalTransportRetainsRecoverableServerFailures(t *testing.T) {
	for _, tc := range []struct {
		code      int
		permanent bool
	}{
		{400, true}, {409, true}, {401, false}, {403, false}, {404, false}, {408, false}, {429, false}, {503, false},
	} {
		t.Run(http.StatusText(tc.code), func(t *testing.T) {
			srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { http.Error(w, "service response", tc.code) }))
			t.Cleanup(srv.Close)
			report, err := terminalreport.NewReport(uuid.NewString(), "57123713200000001", "fail", []byte(`{"error":"provider stopped"}`), time.Now())
			if err != nil {
				t.Fatal(err)
			}
			_, err = NewClient(srv.URL).SendTerminalReport(context.Background(), report)
			var permanent *terminalreport.PermanentError
			if errors.As(err, &permanent) != tc.permanent {
				t.Fatalf("error permanence %v: %v", tc.permanent, err)
			}
		})
	}
}

func TestPermanentCompleteReportFallsBackToFailureBeforeRetiringResult(t *testing.T) {
	var completeCalls, failCalls atomic.Int32
	taskID := uuid.NewString()
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/api/daemon/tasks/" + taskID + "/complete":
			completeCalls.Add(1)
			http.Error(w, "stale terminal receipt", http.StatusBadRequest)
		case "/api/daemon/tasks/" + taskID + "/fail":
			failCalls.Add(1)
			w.WriteHeader(http.StatusOK)
		default:
			http.NotFound(w, r)
		}
	}))
	t.Cleanup(srv.Close)
	root := filepath.Join(t.TempDir(), "reports")
	store, err := terminalreport.Open(root)
	if err != nil {
		t.Fatal(err)
	}
	d := &Daemon{client: NewClient(srv.URL), logger: slog.New(slog.NewTextHandler(io.Discard, nil)), terminalStore: store}
	d.terminalSender = terminalreport.NewSender(store, d.client, d.logger)
	d.terminalSender.SetPermanentRejectionHandler(d.handlePermanentTerminalReport)
	report, err := terminalreport.NewReport(taskID, "57123713200000001", "complete", []byte(`{"output":"finished"}`), time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if _, err := d.terminalStore.Save(report); err != nil {
		t.Fatal(err)
	}
	if err := d.terminalSender.Flush(context.Background()); err != nil {
		t.Fatal(err)
	}
	if completeCalls.Load() != 1 || failCalls.Load() != 1 {
		t.Fatalf("complete/fail fallback calls = %d/%d, want 1/1", completeCalls.Load(), failCalls.Load())
	}
	if pending, _ := d.terminalSender.Stats(); pending != 0 {
		t.Fatal("resolved report remained pending")
	}
	if _, err := os.Stat(filepath.Join(root, taskID+"-57123713200000001.json.rejected")); err != nil {
		t.Fatalf("original report was not retained for diagnosis: %v", err)
	}
}

func TestPermanentFailureReportStaysActionableAndBlocksClaims(t *testing.T) {
	taskID := uuid.NewString()
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Error(w, "stale terminal receipt", http.StatusBadRequest)
	}))
	t.Cleanup(srv.Close)
	root := filepath.Join(t.TempDir(), "reports")
	store, err := terminalreport.Open(root)
	if err != nil {
		t.Fatal(err)
	}
	d := &Daemon{client: NewClient(srv.URL), logger: slog.New(slog.NewTextHandler(io.Discard, nil)), terminalStore: store}
	d.terminalSender = terminalreport.NewSender(store, d.client, d.logger)
	d.terminalSender.SetPermanentRejectionHandler(d.handlePermanentTerminalReport)
	report, err := terminalreport.NewReport(taskID, "57123713200000001", "fail", []byte(`{"error":"provider stopped"}`), time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if _, err := d.terminalStore.Save(report); err != nil {
		t.Fatal(err)
	}
	if err := d.terminalSender.Flush(context.Background()); err == nil {
		t.Fatal("permanently rejected failure report was silently retired")
	}
	if pending, _ := d.terminalSender.Stats(); pending != 1 {
		t.Fatalf("pending reports = %d, want 1", pending)
	}
	if !d.terminalDeliveryBlocked.Load() || d.tryEnterClaim() {
		t.Fatal("blocked terminal report did not stop new claims")
	}
}
