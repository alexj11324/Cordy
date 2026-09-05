//go:build windows

package agent

import (
	"io"
	"log/slog"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/patchbay-ai/patchbay/server/internal/testexec"
)

func TestCursorWindowsCapacityWaitsForWorkerCleanup(t *testing.T) {
	testWindowsCapacityWorkerCleanup(t, "cursor")
}
func TestClaudeWindowsCapacityWaitsForWorkerCleanup(t *testing.T) {
	testWindowsCapacityWorkerCleanup(t, "claude")
}

func testWindowsCapacityWorkerCleanup(t *testing.T, provider string) {
	t.Helper()
	dir := testexec.TempDir(t)
	sourcePath := filepath.Join(dir, "cursor.go")
	exePath := filepath.Join(dir, "cursor.exe")
	pidPath := filepath.Join(dir, "worker.pid")
	source := `package main
import("bufio";"fmt";"io";"os";"os/exec";"time")
func main(){
 if len(os.Args)>1 && os.Args[1]=="worker" {time.Sleep(time.Minute);return}
 if os.Getenv("AGENT_PROVIDER")=="claude" {bufio.NewReader(os.Stdin).ReadString('\n')} else {io.Copy(io.Discard,os.Stdin)}
 child:=exec.Command(os.Args[0],"worker")
 if err:=child.Start();err!=nil{panic(err)}
 if err:=os.WriteFile(os.Getenv("WORKER_PID"),[]byte(fmt.Sprint(child.Process.Pid)),0600);err!=nil{panic(err)}
 fmt.Println("{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"original\"}")
 fmt.Println("{\"type\":\"result\",\"is_error\":true,\"session_id\":\"original\",\"result\":\"API Error: 529 overloaded_error\"}")
}
`
	if err := os.WriteFile(sourcePath, []byte(source), 0600); err != nil {
		t.Fatal(err)
	}
	if output, err := exec.Command("go", "build", "-o", exePath, sourcePath).CombinedOutput(); err != nil {
		t.Fatalf("build fixture: %v: %s", err, output)
	}
	t.Cleanup(func() {
		if raw, err := os.ReadFile(pidPath); err == nil {
			if pid, err := strconv.Atoi(strings.TrimSpace(string(raw))); err == nil && processStillRunning(pid) {
				if p, err := os.FindProcess(pid); err == nil {
					_ = p.Kill()
				}
			}
		}
	})
	b, err := New(provider, Config{ExecutablePath: exePath, Logger: slog.New(slog.NewTextHandler(io.Discard, nil)), Env: map[string]string{"WORKER_PID": pidPath, "AGENT_PROVIDER": provider}})
	if err != nil {
		t.Fatal(err)
	}
	session, err := b.Execute(t.Context(), "original task", ExecOptions{Timeout: 10 * time.Second})
	if err != nil {
		t.Fatal(err)
	}
	for range session.Messages {
	}
	result := <-session.Result
	if result.Status != "failed" || !result.RecoveryResumeSafe {
		t.Fatalf("worker cleanup not confirmed before result: %+v", result)
	}
	raw, err := os.ReadFile(pidPath)
	if err != nil {
		t.Fatal(err)
	}
	pid, err := strconv.Atoi(strings.TrimSpace(string(raw)))
	if err != nil {
		t.Fatal(err)
	}
	if processStillRunning(pid) {
		t.Fatal("owned worker survived capacity recovery boundary")
	}
}
