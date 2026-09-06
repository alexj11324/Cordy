//go:build !windows

package agent

import (
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/patchbay-ai/patchbay/server/internal/testexec"
)

func TestCursorCapacityRequiresCompletePromptWrite(t *testing.T) {
	bin := filepath.Join(testexec.TempDir(t), "cursor-agent")
	// Never read stdin. The prompt exceeds pipe capacity, so its write cannot
	// finish before this terminal response closes the process and the pipe.
	script := `#!/bin/sh
printf '%s\n' '{"type":"system","subtype":"init","session_id":"original"}'
printf '%s\n' '{"type":"result","is_error":true,"session_id":"original","result":"API Error: 529 overloaded_error"}'
exec sleep 30
`
	if err := os.WriteFile(bin, []byte(script), 0700); err != nil {
		t.Fatal(err)
	}
	b, err := New("cursor", Config{ExecutablePath: bin, Logger: slog.New(slog.NewTextHandler(io.Discard, nil))})
	if err != nil {
		t.Fatal(err)
	}
	session, err := b.Execute(t.Context(), strings.Repeat("x", 2<<20), ExecOptions{Timeout: 3 * time.Second})
	if err != nil {
		t.Fatal(err)
	}
	for range session.Messages {
	}
	result := <-session.Result
	if result.Status != "failed" || result.RecoveryResumeSafe {
		t.Fatalf("incomplete prompt must not resume: %+v", result)
	}
}
