package terminalreport

import (
	"context"
	"errors"
	"io"
	"log/slog"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

func testReport(t *testing.T, kind string) Report {
	t.Helper()
	r, err := NewReport(uuid.NewString(), "57123713200000001", kind, []byte(`{"output":"finished code","branch_name":"agent/result"}`), time.Now().Add(-time.Hour))
	if err != nil {
		t.Fatal(err)
	}
	return r
}

func testStore(t *testing.T) *Store {
	t.Helper()
	s, err := Open(filepath.Join(t.TempDir(), "reports"))
	if err != nil {
		t.Fatal(err)
	}
	return s
}

func TestSaveRestartsWithOriginalIdentityAndPrivateFiles(t *testing.T) {
	s := testStore(t)
	r := testReport(t, "complete")
	if _, err := s.Save(r); err != nil {
		t.Fatal(err)
	}
	if runtime.GOOS != "windows" {
		for path, permission := range map[string]os.FileMode{s.root: 0700, filepath.Join(s.root, r.filename()): 0600} {
			stat, err := os.Stat(path)
			if err != nil || stat.Mode().Perm() != permission {
				t.Fatalf("permissions %s: %v, %v", path, stat, err)
			}
		}
	}
	reopened, err := Open(s.root)
	if err != nil {
		t.Fatal(err)
	}
	reports, err := reopened.Pending()
	if err != nil || len(reports) != 1 || reports[0].Identity != r.Identity || string(reports[0].Body) != string(r.Body) {
		t.Fatalf("restart lost report: %+v, %v", reports, err)
	}
	r.Identity.ReportID = uuid.NewString()
	again, err := reopened.Save(r)
	if err != nil || again.Identity != reports[0].Identity {
		t.Fatalf("duplicate save changed identity: %+v, %v", again, err)
	}
	conflict, err := NewReport(r.TaskID, r.Identity.ClaimFence, "fail", []byte(`{"error":"contradiction"}`), time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if _, err := reopened.Save(conflict); err == nil {
		t.Fatal("conflicting result overwrote durable report")
	}
}

func TestCorruptReportIsRetainedAndDoesNotBlockValidReports(t *testing.T) {
	s := testStore(t)
	valid, corrupt := testReport(t, "complete"), testReport(t, "fail")
	for _, r := range []Report{valid, corrupt} {
		if _, err := s.Save(r); err != nil {
			t.Fatal(err)
		}
	}
	path := filepath.Join(s.root, corrupt.filename())
	if err := os.WriteFile(path, []byte(`{"truncated":`), 0600); err != nil {
		t.Fatal(err)
	}
	reports, err := s.Pending()
	if err == nil || len(reports) != 1 || reports[0].TaskID != valid.TaskID {
		t.Fatalf("corrupt report handling = %+v, %v", reports, err)
	}
	if _, err := os.Stat(path + ".corrupt"); err != nil {
		t.Fatalf("corrupt result was discarded: %v", err)
	}
	if _, err := s.Save(corrupt); err == nil {
		t.Fatal("corrupt execution was silently replaced")
	}
}

func TestSaveFailureNeverReportsDurableSuccess(t *testing.T) {
	s := testStore(t)
	if err := os.Remove(s.root); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(s.root, []byte("disk path unavailable"), 0600); err != nil {
		t.Fatal(err)
	}
	if _, err := s.Save(testReport(t, "complete")); err == nil {
		t.Fatal("save unexpectedly succeeded when destination is not writable")
	}
}

func TestTemporaryReadFailureKeepsReportPendingUntilReadable(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("mode-bit read denial is not supported on Windows")
	}
	s := testStore(t)
	r := testReport(t, "complete")
	if _, err := s.Save(r); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(s.root, r.filename())
	if err := os.Chmod(path, 0000); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(path, 0600) })
	if _, err := os.ReadFile(path); err == nil {
		t.Skip("current user can read files despite mode-bit denial")
	}
	for i := 0; i < 2; i++ {
		if _, err := s.Pending(); err == nil {
			t.Fatal("unreadable report allowed unprotected recovery")
		}
		if _, err := os.Stat(path); err != nil {
			t.Fatal("I/O failure removed the durable queue entry", err)
		}
		if _, err := os.Stat(path + ".corrupt"); !errors.Is(err, os.ErrNotExist) {
			t.Fatal("I/O failure mislabeled report corrupt", err)
		}
	}
	if err := os.Chmod(path, 0600); err != nil {
		t.Fatal(err)
	}
	reports, err := s.Pending()
	if err != nil || len(reports) != 1 || reports[0].Identity != r.Identity {
		t.Fatalf("original result did not recover: %+v, %v", reports, err)
	}
}

type transportFunc func(context.Context, Report) (protocol.TerminalReportAck, error)

func (f transportFunc) SendTerminalReport(ctx context.Context, r Report) (protocol.TerminalReportAck, error) {
	return f(ctx, r)
}

func acknowledge(r Report) protocol.TerminalReportAck {
	return protocol.TerminalReportAck{TerminalReportIdentity: r.Identity, TaskID: r.TaskID, Status: "accepted", TaskStatus: "completed"}
}

func newTestSender(s *Store, send transportFunc) *Sender {
	return NewSender(s, send, slog.New(slog.NewTextHandler(io.Discard, nil)))
}

func TestOutageBeyondOldRetryBudgetAndLostResponseRecoverOnRestart(t *testing.T) {
	s := testStore(t)
	r := testReport(t, "complete")
	if _, err := s.Save(r); err != nil {
		t.Fatal(err)
	}
	now := time.Now()
	requests, effects := 0, 0
	accepted := false
	send := transportFunc(func(ctx context.Context, got Report) (protocol.TerminalReportAck, error) {
		requests++
		if got.Identity != r.Identity {
			t.Fatal("retry changed report identity")
		}
		if requests <= 10 {
			return protocol.TerminalReportAck{}, errors.New("network unavailable")
		}
		if !accepted {
			accepted = true
			effects++
			return protocol.TerminalReportAck{}, io.ErrUnexpectedEOF
		}
		return acknowledge(got), nil
	})
	sender := newTestSender(s, send)
	sender.now = func() time.Time { return now }
	for i := 0; i < 11; i++ {
		if err := sender.Flush(context.Background()); err != nil {
			t.Fatal(err)
		}
		now = now.Add(2 * time.Minute)
	}
	if requests != 11 || effects != 1 {
		t.Fatalf("outage requests/effects = %d/%d", requests, effects)
	}
	if pending, _ := sender.Stats(); pending != 1 {
		t.Fatal("unacknowledged report removed")
	}
	// A new store and sender simulate the fresh process state. Only delivery
	// is resumed: neither component has access to a provider runner.
	reopened, err := Open(s.root)
	if err != nil {
		t.Fatal(err)
	}
	fresh := newTestSender(reopened, send)
	if err := fresh.Flush(context.Background()); err != nil {
		t.Fatal(err)
	}
	if requests != 12 || effects != 1 {
		t.Fatalf("replay duplicated side effects: %d requests, %d effects", requests, effects)
	}
	if pending, _ := fresh.Stats(); pending != 0 {
		t.Fatal("acknowledged report still pending")
	}
}

func TestSenderRequiresExactAcknowledgementAndBacksOff(t *testing.T) {
	s := testStore(t)
	r := testReport(t, "complete")
	if _, err := s.Save(r); err != nil {
		t.Fatal(err)
	}
	now := time.Now()
	calls := 0
	sender := newTestSender(s, func(ctx context.Context, r Report) (protocol.TerminalReportAck, error) {
		calls++
		ack := acknowledge(r)
		ack.ClaimFence = "other execution"
		return ack, nil
	})
	sender.now = func() time.Time { return now }
	for i := 0; i < 2; i++ {
		if err := sender.Flush(context.Background()); err != nil {
			t.Fatal(err)
		}
	}
	if calls != 1 {
		t.Fatalf("backoff did not suppress immediate retry: %d", calls)
	}
	if pending, age := sender.Stats(); pending != 1 || age < time.Hour {
		t.Fatalf("bad acknowledgement removed report: pending %d age %s", pending, age)
	}
}

func TestPermanentRejectionRetainsResult(t *testing.T) {
	s := testStore(t)
	r := testReport(t, "fail")
	if _, err := s.Save(r); err != nil {
		t.Fatal(err)
	}
	sender := newTestSender(s, func(ctx context.Context, r Report) (protocol.TerminalReportAck, error) {
		return protocol.TerminalReportAck{}, &PermanentError{Err: errors.New("stale claim")}
	})
	if err := sender.Flush(context.Background()); err != nil {
		t.Fatal(err)
	}
	if pending, _ := sender.Stats(); pending != 0 {
		t.Fatal("permanently rejected result will keep retrying")
	}
	if _, err := os.Stat(filepath.Join(s.root, r.filename()) + ".rejected"); err != nil {
		t.Fatal("rejected report was discarded", err)
	}
}

func TestReportSurvivesProcessExit(t *testing.T) {
	if root := os.Getenv("TERMINAL_REPORT_TEST_CHILD"); root != "" {
		s, err := Open(root)
		if err != nil {
			t.Fatal(err)
		}
		if _, err := s.Save(testReport(t, "complete")); err != nil {
			t.Fatal(err)
		}
		os.Exit(0)
	}
	root := filepath.Join(t.TempDir(), "process-reports")
	cmd := exec.Command(os.Args[0], "-test.run=^TestReportSurvivesProcessExit$")
	cmd.Env = append(os.Environ(), "TERMINAL_REPORT_TEST_CHILD="+root)
	if data, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("reporting process failed: %v, %s", err, data)
	}
	s, err := Open(root)
	if err != nil {
		t.Fatal(err)
	}
	reports, err := s.Pending()
	if err != nil || len(reports) != 1 {
		t.Fatalf("report lost across process exit: %+v, %v", reports, err)
	}
}
