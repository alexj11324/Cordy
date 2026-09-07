package daemon

import (
	"os"
	"path/filepath"
	"runtime"
	"testing"
)

// Automatic discovery and stale-path healing must never execute shell rc files
// or candidate binaries. Both may have arbitrary filesystem side effects.
func TestAgentDiscoveryDoesNotExecuteUserCode(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("POSIX executable fixture")
	}
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("PATH", t.TempDir())
	bin := filepath.Join(home, ".local", "bin")
	if err := os.MkdirAll(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	marker := filepath.Join(home, "executed")
	t.Setenv("DISCOVERY_TEST_MARKER", marker)
	body := []byte("#!/bin/sh\nprintf invoked > \"$DISCOVERY_TEST_MARKER\"\n")
	shell := filepath.Join(home, "bash")
	if err := os.WriteFile(shell, body, 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("SHELL", shell)
	candidate := filepath.Join(bin, "discovery-test-agent")
	if err := os.WriteFile(candidate, body, 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("ORVILO_CLAUDE_PATH", "discovery-test-agent")
	agents := probeAgentCLIs()
	if _, err := os.Stat(marker); !os.IsNotExist(err) {
		t.Fatalf("discovery executed user code: %v", err)
	}
	want, err := filepath.EvalSymlinks(candidate)
	if err != nil {
		t.Fatal(err)
	}
	if agents["claude"].Path != want {
		t.Fatalf("native install not found: got %q, want %q", agents["claude"].Path, want)
	}
	got, ok := reresolveAgentCommand("discovery-test-agent")
	if !ok || got != want {
		t.Fatalf("healing: got %q, %v, want %q", got, ok, want)
	}
	if _, err := os.Stat(marker); !os.IsNotExist(err) {
		t.Fatalf("healing executed user code: %v", err)
	}
}

func stubAgentInstallDirectories(t *testing.T, dirs []string) {
	t.Helper()
	original := agentInstallDirectories
	agentInstallDirectories = func() []string { return dirs }
	t.Cleanup(func() { agentInstallDirectories = original })
}

func TestInstallDiscoveryRefreshesAndSkipsHookWrappers(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("POSIX executable fixture")
	}
	home := t.TempDir()
	t.Setenv("HOME", home)
	first, second := filepath.Join(home, "first"), filepath.Join(home, "second")
	stubAgentInstallDirectories(t, []string{first, second})
	const name = "fixture-agent"
	if len(resolveAgentsFromInstallPaths([]string{name})) != 0 {
		t.Fatal("unexpected initial executable")
	}
	hook := filepath.Join(home, ".orvilo", "hooks", name)
	writeExecStub(t, hook)
	if err := os.MkdirAll(first, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(hook, filepath.Join(first, name)); err != nil {
		t.Fatal(err)
	}
	real := filepath.Join(second, name)
	writeExecStub(t, real)
	got := resolveAgentsFromInstallPaths([]string{name})[name]
	if got != canonicalExecutablePath(real) {
		t.Fatalf("expected fresh real install, not hook wrapper: %q", got)
	}
	if err := os.Remove(real); err != nil {
		t.Fatal(err)
	}
	if len(resolveAgentsFromInstallPaths([]string{name})) != 0 {
		t.Fatal("removed executable remained cached or hook wrapper was accepted")
	}
}
