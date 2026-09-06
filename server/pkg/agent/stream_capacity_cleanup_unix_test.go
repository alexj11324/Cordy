//go:build !windows

package agent

import (
	"context"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/patchbay-ai/patchbay/server/internal/testexec"
)

func TestClaudeCapacityStopsLingeringWorker(t *testing.T) {
	bin := filepath.Join(testexec.TempDir(t), "claude")
	script := `#!/bin/sh
read line
sleep 30 </dev/null >/dev/null 2>&1 &
printf '%s\n' '{"type":"system","subtype":"init","session_id":"original"}'
printf '%s\n' '{"type":"result","is_error":true,"session_id":"original","result":"API Error: 529 overloaded_error"}'
wait
`
	if err := os.WriteFile(bin, []byte(script), 0700); err != nil {
		t.Fatal(err)
	}
	b, err := New("claude", Config{ExecutablePath: bin, Logger: slog.New(slog.NewTextHandler(io.Discard, nil))})
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(t.Context(), 3*time.Second)
	defer cancel()
	session, err := b.Execute(ctx, "task", ExecOptions{})
	if err != nil {
		t.Fatal(err)
	}
	for range session.Messages {
	}
	result := <-session.Result
	if ctx.Err() != nil || result.Status != "failed" || !result.RecoveryResumeSafe {
		t.Fatalf("lingering worker prevented capacity recovery: %+v", result)
	}
}
