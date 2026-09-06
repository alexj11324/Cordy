package agent

import (
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
			bin := filepath.Join(testexec.TempDir(t), "codex")
			script := "#!/bin/sh\n" + `read line
 echo '{"jsonrpc":"2.0","id":1,"result":{}}'
 read line
 read line
 echo '{"jsonrpc":"2.0","id":2,` + reply + `'
 while read line; do
  echo '{"jsonrpc":"2.0","id":3,"result":{"thread":{"id":"unexpected-fresh"}}}'
 done
`
			if err := os.WriteFile(bin, []byte(script), 0700); err != nil {
				t.Fatal(err)
			}
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

func TestCodexCapacityKeepsSafeInitializeRetry(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("POSIX fixture")
	}
	dir := testexec.TempDir(t)
	bin := filepath.Join(dir, "codex")
	script := `#!/bin/sh
if [ "$1" = "--version" ]; then echo 'codex-cli test'; exit; fi
STATE="$(dirname "$0")/attempt"
if [ ! -f "$STATE" ]; then
 touch "$STATE"
 read line
 read ignored
 exit
fi
read line
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{}}'
read line
read line
case "$line" in *thread/resume*original*) ;; *) exit 71;; esac
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"thread":{"id":"original"}}}'
read line
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{}}'
printf '%s\n' '{"jsonrpc":"2.0","method":"turn/started","params":{"threadId":"original","turn":{"id":"turn"}}}'
printf '%s\n' '{"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"original","turn":{"id":"turn","status":"completed"}}}'
cat > /dev/null
`
	if err := os.WriteFile(bin, []byte(script), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(dir, "sessions"), 0700); err != nil {
		t.Fatal(err)
	}
	r, _ := executeFakeCodexCollectingMessagesWithConfig(t, bin, Config{Env: map[string]string{"CODEX_HOME": dir}}, ExecOptions{RequireResume: true, ResumeSessionID: "original", HandshakeTimeout: time.Second, Timeout: 5 * time.Second}, 10*time.Second)
	if r.Status != "completed" || r.SessionID != "original" {
		t.Fatalf("safe initialization retry was suppressed: %+v", r)
	}
}
