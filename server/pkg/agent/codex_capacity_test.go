package agent

import (
	"io"
	"log/slog"
	"runtime"
	"strings"
	"testing"
	"time"
)

func TestCodexCapacityErrorMetadata(t *testing.T) {
	for _, method := range []string{"error", "turn/completed"} {
		t.Run(method, func(t *testing.T) {
			c := &codexClient{cfg: Config{Logger: slog.New(slog.NewTextHandler(io.Discard, nil))}, notificationProtocol: "raw"}
			params := `{"error":{"message":"busy","codexErrorInfo":"usageLimitExceeded"},"willRetry":false}`
			if method == "turn/completed" {
				params = `{"turn":{"id":"t","status":"failed","error":{"message":"busy","codexErrorInfo":"usageLimitExceeded"}}}`
			}
			c.handleLine(`{"jsonrpc":"2.0","method":"` + method + `","params":` + params + `}`)
			if c.getTurnErrorCode() != "usageLimitExceeded" {
				t.Fatalf("code=%q", c.getTurnErrorCode())
			}
		})
	}
	t.Run("retry notification ignored", func(t *testing.T) {
		c := &codexClient{cfg: Config{Logger: slog.Default()}, notificationProtocol: "raw"}
		c.handleLine(`{"jsonrpc":"2.0","method":"error","params":{"error":{"message":"busy","codexErrorInfo":"serverOverloaded"},"willRetry":true}}`)
		if c.getTurnErrorCode() != "" || c.getTurnError() != "" {
			t.Fatal("upstream retry became terminal")
		}
	})
	t.Run("terminal quota supersedes earlier capacity", func(t *testing.T) {
		c := &codexClient{cfg: Config{Logger: slog.Default()}, notificationProtocol: "raw"}
		c.handleLine(`{"jsonrpc":"2.0","method":"error","params":{"error":{"message":"busy","codexErrorInfo":"serverOverloaded"},"willRetry":false}}`)
		c.handleLine(`{"jsonrpc":"2.0","method":"turn/completed","params":{"turn":{"status":"failed","error":{"message":"quota exhausted","codexErrorInfo":"usageLimitExceeded"}}}}`)
		if c.getTurnErrorCode() != "usageLimitExceeded" || c.getTurnError() != "quota exhausted" {
			t.Fatalf("wrong terminal error: %q %q", c.getTurnErrorCode(), c.getTurnError())
		}
	})
}

func TestCodexCapacityStrictResume(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("POSIX fixture")
	}
	for _, response := range []string{`{"error":{"code":-32000,"message":"thread not found"}}`, `{"result":{"thread":{"id":"different"}}}`, `{"result":{}}`} {
		t.Run(response, func(t *testing.T) {
			reply := strings.TrimPrefix(response, "{")
			bin := writeFakeCodexAppServer(t, `read line
 echo '{"jsonrpc":"2.0","id":1,"result":{}}'
 read line
 read line
 echo '{"jsonrpc":"2.0","id":2,`+reply+`'
 while read line; do
  echo '{"jsonrpc":"2.0","id":3,"result":{"thread":{"id":"unexpected-fresh"}}}'
 done
`)
			r := executeFakeCodex(t, bin, ExecOptions{RequireResume: true, ResumeSessionID: "original", Timeout: 3 * time.Second})
			if r.Status != "failed" || r.SessionID != "" {
				t.Fatalf("unexpected fresh execution: %+v", r)
			}
			if !strings.Contains(r.Error, "resume") {
				t.Fatalf("wrong failure: %+v", r)
			}
		})
	}
}
