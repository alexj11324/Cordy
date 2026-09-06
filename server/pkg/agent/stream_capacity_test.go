package agent

import (
	"context"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/patchbay-ai/patchbay/server/internal/testexec"
)

// Run only a test-owned executable, never a user-installed coding agent.
func runCapacityStreamFixture(t *testing.T, provider, frames string, opts ExecOptions) (Result, []Message) {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("POSIX shell fixture")
	}
	root := testexec.TempDir(t)
	bin := filepath.Join(root, provider)
	readInput := "read line\n"
	if provider == "cursor" {
		readInput = "cat > /dev/null\n"
	}
	script := "#!/bin/sh\n" + readInput + "cat <<'FRAMES'\n" + frames + "\nFRAMES\n"
	if err := os.WriteFile(bin, []byte(script), 0700); err != nil {
		t.Fatal(err)
	}
	b, err := New(provider, Config{ExecutablePath: bin, Logger: slog.New(slog.NewTextHandler(io.Discard, nil)), Env: map[string]string{"HOME": root}})
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	s, err := b.Execute(ctx, "test", opts)
	if err != nil {
		return Result{Status: "failed", Error: err.Error()}, nil
	}
	var messages []Message
	for m := range s.Messages {
		messages = append(messages, m)
	}
	return <-s.Result, messages
}

func TestStreamCapacityCleanupEvidence(t *testing.T) {
	for _, provider := range []string{"claude", "cursor"} {
		t.Run(provider, func(t *testing.T) {
			frames := `{"type":"system","subtype":"init","session_id":"original"}` + "\n" + `{"type":"result","is_error":true,"session_id":"original","result":"API Error: 529 overloaded_error","inputTokens":100}`
			result, _ := runCapacityStreamFixture(t, provider, frames, ExecOptions{})
			if result.UsageCumulative != (provider == "cursor") {
				t.Fatalf("wrong usage basis: %+v", result)
			}
			if result.Status != "failed" || result.SessionID != "original" || !result.RecoveryResumeSafe {
				t.Fatalf("no safe recovery evidence: %+v", result)
			}
		})
	}
}

func TestStreamCapacityRequiresOriginalSession(t *testing.T) {
	for _, provider := range []string{"claude", "cursor"} {
		for _, sid := range []string{"different", ""} {
			t.Run(provider+"/"+sid, func(t *testing.T) {
				frames := `{"type":"system","subtype":"init","session_id":"` + sid + `"}` + "\n" + `{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"wrong conversation"}]}}` + "\n" + `{"type":"result","session_id":"` + sid + `","result":"done"}`
				result, messages := runCapacityStreamFixture(t, provider, frames, ExecOptions{ResumeSessionID: "original", RequireResume: true})
				if result.Status != "failed" || !strings.Contains(result.Error, "recovery") || result.RecoveryResumeSafe {
					t.Fatalf("lost continuity accepted: %+v", result)
				}
				for _, m := range messages {
					if m.Type == MessageText || m.Type == MessageToolUse || m.SessionID == "different" {
						t.Fatalf("wrong session leaked: %+v", m)
					}
				}
			})
		}
	}
}

func TestStreamCapacityOriginalSessionCompletes(t *testing.T) {
	for _, provider := range []string{"claude", "cursor"} {
		t.Run(provider, func(t *testing.T) {
			frames := `{"type":"system","subtype":"init","session_id":"original"}` + "\n" + `{"type":"result","session_id":"original","result":"done"}`
			result, _ := runCapacityStreamFixture(t, provider, frames, ExecOptions{ResumeSessionID: "original", RequireResume: true})
			if result.Status != "completed" || result.SessionID != "original" {
				t.Fatalf("valid resume failed: %+v", result)
			}
		})
	}
}

func TestClaudeCapacityStructuredFailure(t *testing.T) {
	for _, tc := range []struct{ name, frames, code, text string }{
		{"native rate limit", `{"type":"assistant","error":"rate_limit","session_id":"original","message":{"role":"assistant","content":[{"type":"text","text":"429"}]}}` + "\n" + `{"type":"result","is_error":true,"session_id":"original","result":"429"}`, "rate_limit", "429"},
		{"terminal error array", `{"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"original","errors":["API Error: 529 overloaded_error"]}`, "", "overloaded_error"},
		{"budget beats overloaded", `{"type":"assistant","error":"overloaded","session_id":"original","message":{"role":"assistant","content":[]}}` + "\n" + `{"type":"result","subtype":"error_max_budget_usd","is_error":true,"session_id":"original","errors":["budget exceeded"]}`, "billing_error", "budget exceeded"},
		{"quota beats prior overloaded", `{"type":"assistant","error":"overloaded","session_id":"original","message":{"role":"assistant","content":[]}}` + "\n" + `{"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"original","errors":["usage limit reached"]}`, "billing_error", "usage limit"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			r, _ := runCapacityStreamFixture(t, "claude", `{"type":"system","subtype":"init","session_id":"original"}`+"\n"+tc.frames, ExecOptions{})
			if r.Status != "failed" || r.ProviderErrorCode != tc.code || !strings.Contains(r.Error, tc.text) {
				t.Fatalf("structured failure lost: %+v", r)
			}
		})
	}
}

func TestClaudeCapacityNativeRateLimitCanMeanQuota(t *testing.T) {
	frames := `{"type":"system","subtype":"init","session_id":"original"}` + "\n" +
		`{"type":"assistant","error":"rate_limit","session_id":"original","message":{"role":"assistant","content":[]}}` + "\n" +
		`{"type":"result","is_error":true,"session_id":"original","result":"You've hit your limit. Try again tomorrow."}`
	r, _ := runCapacityStreamFixture(t, "claude", frames, ExecOptions{})
	if r.ProviderErrorCode != "billing_error" {
		t.Fatalf("quota must override broad CC rate_limit code: %+v", r)
	}
}

func TestStreamCapacityRejectsConflictingOptions(t *testing.T) {
	for _, provider := range []string{"claude", "cursor"} {
		for _, args := range [][]string{{"--resume=other"}, {"'--resume=other'"}, {"--continue"}, {"--worktree"}, {"--workspace", "other"}, {"--fork-session"}} {
			t.Run(provider+strings.Join(args, "/"), func(t *testing.T) {
				r, _ := runCapacityStreamFixture(t, provider, `{"type":"result","session_id":"original","result":"should not launch"}`, ExecOptions{RequireResume: true, ResumeSessionID: "original", CustomArgs: args})
				if r.Status != "failed" || !strings.Contains(r.Error, "custom") {
					t.Fatalf("conflicting flags accepted: %+v", r)
				}
			})
		}
	}
}

func TestCursorCapacityErrorEventAfterInit(t *testing.T) {
	frames := `{"type":"system","subtype":"init","session_id":"original"}` + "\n" + `{"type":"error","error":"API Error: 529 overloaded_error"}`
	r, messages := runCapacityStreamFixture(t, "cursor", frames, ExecOptions{})
	if r.Status != "failed" || !r.RecoveryResumeSafe || r.SessionID != "original" {
		t.Fatalf("explicit terminal error cannot recover: %+v", r)
	}
	pinned := false
	for _, m := range messages {
		if m.Type == MessageStatus && m.SessionID == "original" {
			pinned = true
		}
	}
	if !pinned {
		t.Fatal("original session must be pinned before waiting")
	}
}

func TestClaudeCapacityBlockingErrorSurvivesLaterAssistant(t *testing.T) {
	for _, later := range []string{"rate_limit", ""} {
		t.Run(later, func(t *testing.T) {
			frames := `{"type":"system","subtype":"init","session_id":"original"}` + "\n" +
				`{"type":"assistant","error":"billing_error","session_id":"original","message":{"role":"assistant","content":[]}}` + "\n" +
				`{"type":"assistant","error":"` + later + `","session_id":"original","message":{"role":"assistant","content":[]}}` + "\n" +
				`{"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"original","errors":["rate limit reached"]}`
			r, _ := runCapacityStreamFixture(t, "claude", frames, ExecOptions{})
			if r.ProviderErrorCode != "billing_error" {
				t.Fatalf("definite blocking error lost: %+v", r)
			}
		})
	}
}

func TestClaudeCapacityPreservesTypedErrorWithGenericArray(t *testing.T) {
	for _, code := range []string{"rate_limit", "overloaded"} {
		t.Run(code, func(t *testing.T) {
			frames := `{"type":"system","subtype":"init","session_id":"original"}` + "\n" +
				`{"type":"assistant","error":"` + code + `","session_id":"original","message":{"role":"assistant","content":[]}}` + "\n" +
				`{"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"original","errors":["HTTP 429"]}`
			r, _ := runCapacityStreamFixture(t, "claude", frames, ExecOptions{})
			if r.ProviderErrorCode != code {
				t.Fatalf("typed transient failure was erased: %+v", r)
			}
		})
	}
}

func TestClaudeCapacityFinalNonTransientErrorOverridesMetadata(t *testing.T) {
	for _, message := range []string{"model not found", "context length exceeded", "missing API key"} {
		t.Run(message, func(t *testing.T) {
			frames := `{"type":"system","subtype":"init","session_id":"original"}` + "\n" +
				`{"type":"assistant","error":"overloaded","session_id":"original","message":{"role":"assistant","content":[]}}` + "\n" +
				`{"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"original","errors":["` + message + `"]}`
			r, _ := runCapacityStreamFixture(t, "claude", frames, ExecOptions{})
			if r.ProviderErrorCode == "overloaded" || r.ProviderErrorCode == "rate_limit" {
				t.Fatalf("terminal non-transient error became retryable: %+v", r)
			}
		})
	}
}

func TestClaudeCapacityPermanentArrayEntryOverridesEarlierLimit(t *testing.T) {
	frames := `{"type":"system","subtype":"init","session_id":"original"}` + "\n" +
		`{"type":"assistant","error":"overloaded","session_id":"original","message":{"role":"assistant","content":[]}}` + "\n" +
		`{"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"original","errors":["rate limit reached","model not found"]}`
	r, _ := runCapacityStreamFixture(t, "claude", frames, ExecOptions{})
	if r.ProviderErrorCode == "overloaded" || r.ProviderErrorCode == "rate_limit" {
		t.Fatalf("earlier transient array entry masked terminal failure: %+v", r)
	}
}
