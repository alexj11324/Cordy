package daemon

import (
	"context"
	"sync/atomic"
	"testing"
	"time"

	"github.com/patchbay-ai/patchbay/server/pkg/agent"
	"github.com/patchbay-ai/patchbay/server/pkg/taskfailure"
)

func capacityResult(code, message string) agent.Result {
	return agent.Result{Status: "failed", SessionID: "original", Error: message, ProviderErrorCode: code, RecoveryResumeSafe: true}
}
func TestCapacityRecoveryEligibility(t *testing.T) {
	for _, tc := range []struct {
		code, message string
		want          bool
	}{
		{"serverOverloaded", "busy", true}, {"rateLimitExceeded", "429", true},
		{"usageLimitExceeded", "rate limit 429", false}, {"unauthorized", "Selected model is at capacity", false},
		{"other", "Selected model is at capacity", false}, {"", "429", false},
		{"", "Selected model is at capacity. Please try a different model.", true},
		{"", "rate limit reached", true},
		{"", "API Error: 529 overloaded_error", true},
		{"", "API Error: 529 overloaded", true},
		{"", "rate_limit_error: retry later", true},
		{"", "You have exceeded your usage limit", false},
		{"", "RESOURCE_EXHAUSTED", false}, {"", "429 insufficient_quota", false},
		{"", "503 service unavailable", false}, {"", "process crashed", false},
	} {
		t.Run(tc.code+tc.message, func(t *testing.T) {
			if got := canRecoverCapacity(capacityResult(tc.code, tc.message)); got != tc.want {
				t.Fatalf("got %v want %v", got, tc.want)
			}
		})
	}
	r := capacityResult("serverOverloaded", "busy")
	r.RecoveryResumeSafe = false
	if canRecoverCapacity(r) {
		t.Fatal("cleanup not confirmed")
	}
	r.RecoveryResumeSafe = true
	r.SessionID = ""
	if canRecoverCapacity(r) {
		t.Fatal("no thread")
	}
	r.SessionID = "original"
	r.Status = "cancelled"
	if canRecoverCapacity(r) {
		t.Fatal("cancelled")
	}
}
func TestCapacityRecoveryContinuesSameTask(t *testing.T) {
	d := newTestDaemon(t)
	first := capacityResult("serverOverloaded", "busy")
	first.Usage = map[string]agent.TokenUsage{"model": {InputTokens: 2}}
	b := &fakeBackend{}
	for range 6 {
		b.results = append(b.results, capacityResult("rateLimitExceeded", "busy"))
	}
	b.results = append(b.results, agent.Result{Status: "completed", SessionID: "original", Output: "done", Usage: map[string]agent.TokenUsage{"model": {InputTokens: 3}}})
	var delays []time.Duration
	var seq atomic.Int32
	r, _, err := d.recoverCapacity(context.Background(), b, first, 0, agent.ExecOptions{Cwd: "kept"}, d.logger, "task", "", &seq, func(_ context.Context, delay time.Duration) error { delays = append(delays, delay); return nil })
	if err != nil || r.Status != "completed" || r.Output != "done" || r.Usage["model"].InputTokens != 5 {
		t.Fatalf("result=%+v err=%v", r, err)
	}
	want := []time.Duration{15 * time.Second, 30 * time.Second, time.Minute, 2 * time.Minute, 5 * time.Minute, 5 * time.Minute, 5 * time.Minute}
	if len(delays) != len(want) {
		t.Fatalf("delays=%v", delays)
	}
	for i, v := range want {
		if delays[i] != v {
			t.Fatalf("delay[%d]=%s", i, delays[i])
		}
	}
	for _, o := range b.calls {
		if !o.RequireResume || o.ResumeSessionID != "original" || o.Cwd != "kept" {
			t.Fatalf("continuity=%+v", o)
		}
	}
}
func TestCapacityRecoveryCancelAndQuota(t *testing.T) {
	for _, cancelWait := range []bool{true, false} {
		t.Run(map[bool]string{true: "cancel", false: "quota"}[cancelWait], func(t *testing.T) {
			d := newTestDaemon(t)
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			b := &fakeBackend{results: []agent.Result{capacityResult("usageLimitExceeded", "quota exhausted")}}
			var seq atomic.Int32
			r, _, err := d.recoverCapacity(ctx, b, capacityResult("serverOverloaded", "busy"), 0, agent.ExecOptions{}, d.logger, "task", "", &seq, func(ctx context.Context, _ time.Duration) error {
				if cancelWait {
					cancel()
					return sleepWithContext(ctx, time.Hour)
				}
				return nil
			})
			if err != nil {
				t.Fatal(err)
			}
			if cancelWait {
				if r.Status != "cancelled" || len(b.calls) != 0 {
					t.Fatalf("cancel failed: %+v", r)
				}
			} else if r.ProviderErrorCode != "usageLimitExceeded" || len(b.calls) != 1 {
				t.Fatalf("quota retried: %+v", r)
			}
		})
	}
}

func TestCapacityRecoveryDoesNotSumCursorSessionTotals(t *testing.T) {
	d := newTestDaemon(t)
	first := capacityResult("serverOverloaded", "busy")
	first.Usage = map[string]agent.TokenUsage{"model": {InputTokens: 100}}
	first.UsageCumulative = true
	next := capacityResult("serverOverloaded", "busy")
	next.Usage = map[string]agent.TokenUsage{"model": {InputTokens: 150}}
	next.UsageCumulative = true
	b := &fakeBackend{results: []agent.Result{next, {Status: "completed", SessionID: "original", UsageCumulative: true, Usage: map[string]agent.TokenUsage{"model": {InputTokens: 160}}}}}
	var seq atomic.Int32
	r, _, err := d.recoverCapacity(context.Background(), b, first, 0, agent.ExecOptions{}, d.logger, "task", "", &seq, func(context.Context, time.Duration) error { return nil })
	if err != nil || r.Usage["model"].InputTokens != 160 {
		t.Fatalf("cumulative usage was added: %+v err=%v", r, err)
	}
}

func TestCapacityRecoveryRetainsUsageBeforeFreshFallback(t *testing.T) {
	d := newTestDaemon(t)
	first := agent.Result{Status: "failed", SessionID: "old", Usage: map[string]agent.TokenUsage{"model": {InputTokens: 100}}}
	fresh := capacityResult("serverOverloaded", "busy")
	fresh.UsageCumulative = true
	fresh.Usage = map[string]agent.TokenUsage{"model": {InputTokens: 150}}
	combined, tools := reconcileFreshRetryResult(first, first.Usage, 0, fresh, 0, nil)
	b := &fakeBackend{results: []agent.Result{{Status: "completed", SessionID: "original", UsageCumulative: true, Usage: map[string]agent.TokenUsage{"model": {InputTokens: 160}}}}}
	var seq atomic.Int32
	r, _, err := d.recoverCapacity(context.Background(), b, combined, tools, agent.ExecOptions{}, d.logger, "task", "", &seq, func(context.Context, time.Duration) error { return nil })
	if err != nil || r.Usage["model"].InputTokens != 260 {
		t.Fatalf("prior-session usage lost: %+v err=%v", r, err)
	}
}

func TestCapacityFailureReasonPreservesTerminalArrayClassification(t *testing.T) {
	r := agent.Result{ProviderErrorCode: taskfailure.ReasonAgentModelNotFoundOrUnavailable.String(), Error: "rate limit reached\nmodel not found"}
	if got := providerFailureReason(r); got != taskfailure.ReasonAgentModelNotFoundOrUnavailable {
		t.Fatalf("got %s", got)
	}
}
