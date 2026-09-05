//go:build !windows

package daemon

import (
	"context"
	"encoding/json"
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

	"github.com/patchbay-ai/patchbay/server/pkg/agent"
)

// Drive real Codex adapter subprocesses and daemon message reporting. Only the
// upstream model is simulated; no installed CLI or account is accessed.
func TestCapacityRecoveryProtocolRuntime(t *testing.T) {
	root := t.TempDir()
	// Keep usage discovery inside the fixture rather than the user's history.
	if err := os.MkdirAll(filepath.Join(root, "home", "sessions"), 0700); err != nil {
		t.Fatal(err)
	}
	bin := filepath.Join(root, "codex")
	script := `#!/bin/sh
set -eu
STATE="$(dirname "$0")"
mkdir "$STATE/active" || exit 71
trap 'rmdir "$STATE/active"' EXIT
count=0
if [ -f "$STATE/count" ]; then count=$(cat "$STATE/count"); fi
count=$((count+1)); echo "$count" > "$STATE/count"
read line
echo '{"jsonrpc":"2.0","id":1,"result":{}}'
read line
read line
printf '%s\n' "$line" >> "$STATE/requests"
if [ "$count" -gt 1 ]; then
 case "$line" in *thread/resume*original*) ;; *) exit 72;; esac
fi
echo '{"jsonrpc":"2.0","id":2,"result":{"thread":{"id":"original"}}}'
read line
printf '%s\n' "$line" >> "$STATE/requests"
echo '{"jsonrpc":"2.0","id":3,"result":{}}'
echo '{"jsonrpc":"2.0","method":"turn/started","params":{"threadId":"original","turn":{"id":"turn"}}}'
if [ "$count" -eq 1 ]; then
 echo done > "$STATE/completed-work"
 echo '{"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"original","item":{"type":"agentMessage","id":"first","text":"First part completed."}}}'
fi
if [ "$count" -lt 3 ]; then
 echo '{"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"original","turn":{"id":"turn","status":"failed","error":{"message":"Selected model is at capacity. Please try a different model.","codexErrorInfo":"serverOverloaded"}}}}'
else
 test -f "$STATE/completed-work" || exit 73
 echo '{"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"original","item":{"type":"agentMessage","id":"final","text":"Task resumed and completed."}}}'
 echo '{"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"original","turn":{"id":"turn","status":"completed"}}}'
fi
# Wait for the adapter to close stdin; process cleanup must precede recovery.
cat > /dev/null
`
	if err := os.WriteFile(bin, []byte(script), 0700); err != nil {
		t.Fatal(err)
	}
	var mu sync.Mutex
	var messages []TaskMessageData
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.HasSuffix(r.URL.Path, "/messages") {
			var batch struct {
				Messages []TaskMessageData `json:"messages"`
			}
			if err := json.NewDecoder(r.Body).Decode(&batch); err != nil {
				t.Error(err)
			}
			mu.Lock()
			messages = append(messages, batch.Messages...)
			mu.Unlock()
		}
		w.WriteHeader(http.StatusOK)
	}))
	defer srv.Close()
	logger := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelDebug}))
	d := &Daemon{client: NewClient(srv.URL), logger: logger}
	backend, err := agent.New("codex", agent.Config{ExecutablePath: bin, Logger: logger, Env: map[string]string{"CODEX_HOME": filepath.Join(root, "home")}})
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	opts := agent.ExecOptions{Cwd: root, Timeout: 3 * time.Second}
	var seq atomic.Int32
	first, tools, err := d.executeAndDrain(ctx, backend, "Original task, execute once", opts, logger, "task", "", &seq)
	if err != nil {
		t.Fatal(err)
	}
	if !canRecoverCapacity(first) {
		t.Fatalf("capacity not recoverable: %+v", first)
	}
	waits := 0
	result, _, err := d.recoverCapacity(ctx, backend, first, tools, opts, logger, "task", "", &seq, func(_ context.Context, delay time.Duration) error {
		waits++
		if _, err := os.Stat(filepath.Join(root, "active")); !os.IsNotExist(err) {
			t.Fatal("previous child still active")
		}
		if delay != capacityRecoveryDelay(waits-1) {
			t.Fatalf("delay=%s", delay)
		}
		return nil
	})
	if err != nil || result.Status != "completed" || result.Output != "Task resumed and completed." || result.SessionID != "original" || waits != 2 {
		t.Fatalf("result=%+v waits=%d err=%v", result, waits, err)
	}
	requests, err := os.ReadFile(filepath.Join(root, "requests"))
	if err != nil {
		t.Fatal(err)
	}
	if strings.Count(string(requests), "Original task, execute once") != 1 || strings.Count(string(requests), "Continue the interrupted task") != 2 {
		t.Fatalf("prompt replay: %s", requests)
	}
	mu.Lock()
	defer mu.Unlock()
	notices := 0
	lastSeq := 0
	var text string
	for _, m := range messages {
		if m.Seq <= lastSeq {
			t.Fatalf("non-monotonic sequence: %+v", messages)
		}
		lastSeq = m.Seq
		if m.Type == "thinking" && strings.Contains(m.Content, "retrying in") {
			notices++
		}
		if m.Type == "text" {
			text += m.Content
		}
	}
	if notices != 2 || !strings.Contains(text, "First part completed.") || !strings.Contains(text, "Task resumed and completed.") {
		t.Fatalf("transcript=%+v", messages)
	}
}
