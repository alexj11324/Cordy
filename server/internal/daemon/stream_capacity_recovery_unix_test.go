//go:build !windows

package daemon

import (
	"context"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync/atomic"
	"syscall"
	"testing"
	"time"

	"github.com/patchbay-ai/patchbay/server/internal/testexec"

	"github.com/patchbay-ai/patchbay/server/pkg/agent"
	"github.com/patchbay-ai/patchbay/server/pkg/taskfailure"
)

func TestStreamCapacityRecoveryProtocol(t *testing.T) {
	for _, provider := range []string{"claude", "cursor"} {
		for _, outcome := range []string{"success", "quota", "bare429", "cancel"} {
			t.Run(provider+"/"+outcome, func(t *testing.T) {
				root := testexec.TempDir(t)
				bin := filepath.Join(root, provider)
				script := `#!/bin/sh
set -eu
STATE="$(dirname "$0")"
count=0
if [ -f "$STATE/count" ]; then count=$(cat "$STATE/count"); fi
count=$((count+1)); echo "$count" > "$STATE/count"
echo $$ > "$STATE/pid"
line=''; IFS= read -r line || true
printf '%s\n' "$line" >> "$STATE/prompts"
if [ "$count" -eq 1 ]; then echo completed > "$STATE/work"; else
 case " $* " in *" --resume original "*) ;; *) exit 70;; esac
 test -f "$STATE/work" || exit 71
fi
printf '%s\n' '{"type":"system","subtype":"init","session_id":"original"}'
if [ "$OUTCOME" = bare429 ]; then
 printf '%s\n' '{"type":"result","is_error":true,"session_id":"original","result":"HTTP 429"}'
elif [ "$count" -gt 1 ] && [ "$OUTCOME" = quota ]; then
 printf '%s\n' '{"type":"result","is_error":true,"session_id":"original","result":"usage limit reached"}'
elif [ "$count" -lt 3 ]; then
 printf '%s\n' '{"type":"result","is_error":true,"session_id":"original","result":"API Error: 529 overloaded_error"}'
else
 printf '%s\n' '{"type":"result","is_error":false,"session_id":"original","result":"Recovered work"}'
fi
`
				if err := os.WriteFile(bin, []byte(script), 0700); err != nil {
					t.Fatal(err)
				}
				d := newTestDaemon(t)
				logger := slog.New(slog.NewTextHandler(io.Discard, nil))
				b, err := agent.New(provider, agent.Config{ExecutablePath: bin, Logger: logger, Env: map[string]string{"HOME": root, "OUTCOME": outcome}})
				if err != nil {
					t.Fatal(err)
				}
				ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
				defer cancel()
				opts := agent.ExecOptions{Cwd: root, Timeout: 3 * time.Second}
				var seq atomic.Int32
				first, tools, err := d.executeAndDrain(ctx, b, "Original instruction", opts, logger, "task", "", &seq)
				if err != nil {
					t.Fatal(err)
				}
				waits := 0
				result, _, err := d.recoverCapacity(ctx, b, first, tools, opts, logger, "task", "", &seq, func(ctx context.Context, delay time.Duration) error {
					data, err := os.ReadFile(filepath.Join(root, "pid"))
					if err != nil {
						t.Fatal(err)
					}
					pid, err := strconv.Atoi(strings.TrimSpace(string(data)))
					if err != nil {
						t.Fatal(err)
					}
					if err := syscall.Kill(pid, 0); err != syscall.ESRCH {
						t.Fatalf("previous process still exists: %d %v", pid, err)
					}
					if delay != capacityRecoveryDelay(waits) {
						t.Fatalf("delay %s", delay)
					}
					waits++
					if outcome == "cancel" {
						cancel()
						return ctx.Err()
					}
					return nil
				})
				if err != nil {
					t.Fatal(err)
				}
				wantLaunches := 1
				switch outcome {
				case "success":
					wantLaunches = 3
					if result.Status != "completed" || result.Output != "Recovered work" || waits != 2 {
						t.Fatalf("recovery=%+v waits=%d", result, waits)
					}
				case "quota":
					wantLaunches = 2
					if result.Status != "failed" || providerFailureReason(result) != taskfailure.ReasonAgentProviderQuotaLimit || waits != 1 {
						t.Fatalf("quota=%+v waits=%d", result, waits)
					}
				case "bare429":
					if result.Status != "failed" || waits != 0 {
						t.Fatalf("ambiguous 429 retried: %+v", result)
					}
				case "cancel":
					if result.Status != "cancelled" || waits != 1 {
						t.Fatalf("cancel=%+v", result)
					}
				}
				data, err := os.ReadFile(filepath.Join(root, "count"))
				if err != nil {
					t.Fatal(err)
				}
				if strings.TrimSpace(string(data)) != strconv.Itoa(wantLaunches) {
					t.Fatalf("launch count=%s want=%d", data, wantLaunches)
				}
				prompts, err := os.ReadFile(filepath.Join(root, "prompts"))
				if err != nil {
					t.Fatal(err)
				}
				if strings.Count(string(prompts), "Original instruction") != 1 || strings.Count(string(prompts), "Continue the interrupted task") != wantLaunches-1 {
					t.Fatalf("prompt replay: %s", prompts)
				}
				if result.SessionID != "original" {
					t.Fatalf("lost original session: %+v", result)
				}
			})
		}
	}
}
