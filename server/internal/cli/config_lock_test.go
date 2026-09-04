package cli

import (
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

const configLockHelperEnv = "PATCHBAY_TEST_CONFIG_LOCK_HELPER"

func TestConfigLockTimesOutWithoutDeletingOrTruncatingHeldLock(t *testing.T) {
	path := filepath.Join(t.TempDir(), ".config.lock")
	if err := os.WriteFile(path, []byte("lock-sentinel"), 0o644); err != nil {
		t.Fatal(err)
	}
	first, err := acquireConfigLock(path, time.Second)
	if err != nil {
		t.Fatal(err)
	}
	defer first.Close()
	defer unlockConfigFile(first) //nolint:errcheck -- test cleanup

	started := time.Now()
	second, err := acquireConfigLock(path, 75*time.Millisecond)
	if second != nil {
		_ = second.Close()
		t.Fatal("second lock unexpectedly acquired")
	}
	if err == nil || !strings.Contains(err.Error(), "timed out") {
		t.Fatalf("second lock error = %v, want timeout", err)
	}
	if elapsed := time.Since(started); elapsed < 50*time.Millisecond {
		t.Fatalf("lock timeout returned too early: %s", elapsed)
	}
	contents, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if string(contents) != "lock-sentinel" {
		t.Fatalf("lock file was truncated: %q", contents)
	}
}

func TestConfigLockReusesStaleUnlockedFile(t *testing.T) {
	path := filepath.Join(t.TempDir(), ".config.lock")
	if err := os.WriteFile(path, []byte("left by exited process"), 0o644); err != nil {
		t.Fatal(err)
	}
	lock, err := acquireConfigLock(path, time.Second)
	if err != nil {
		t.Fatalf("acquire stale lock file: %v", err)
	}
	if err := unlockConfigFile(lock); err != nil {
		t.Fatal(err)
	}
	if err := lock.Close(); err != nil {
		t.Fatal(err)
	}
	contents, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if string(contents) != "left by exited process" {
		t.Fatalf("stale lock file was modified: %q", contents)
	}
	if runtime.GOOS != "windows" && testFileMode(t, path) != 0o600 {
		t.Fatalf("lock mode = %#o, want 0600", testFileMode(t, path))
	}
}

func TestConfigLockCrossProcessCrashRecovery(t *testing.T) {
	if os.Getenv(configLockHelperEnv) == "1" {
		path := os.Getenv("PATCHBAY_TEST_CONFIG_LOCK_PATH")
		ready := os.Getenv("PATCHBAY_TEST_CONFIG_LOCK_READY")
		lock, err := acquireConfigLock(path, time.Second)
		if err != nil {
			os.Exit(2)
		}
		defer lock.Close()
		if err := os.WriteFile(ready, []byte("ready"), 0o600); err != nil {
			os.Exit(3)
		}
		time.Sleep(30 * time.Second)
		return
	}

	dir := t.TempDir()
	path := filepath.Join(dir, ".config.lock")
	ready := filepath.Join(dir, "ready")
	command := exec.Command(os.Args[0], "-test.run=^TestConfigLockCrossProcessCrashRecovery$")
	command.Env = append(os.Environ(),
		configLockHelperEnv+"=1",
		"PATCHBAY_TEST_CONFIG_LOCK_PATH="+path,
		"PATCHBAY_TEST_CONFIG_LOCK_READY="+ready,
	)
	if err := command.Start(); err != nil {
		t.Fatal(err)
	}
	waited := false
	defer func() {
		if !waited {
			_ = command.Process.Kill()
			_ = command.Wait()
		}
	}()

	deadline := time.Now().Add(3 * time.Second)
	for {
		if _, err := os.Stat(ready); err == nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("lock helper did not become ready")
		}
		time.Sleep(10 * time.Millisecond)
	}
	if lock, err := acquireConfigLock(path, 75*time.Millisecond); err == nil {
		_ = unlockConfigFile(lock)
		_ = lock.Close()
		t.Fatal("parent acquired lock while child held it")
	}
	if err := command.Process.Kill(); err != nil {
		t.Fatal(err)
	}
	if err := command.Wait(); err == nil {
		t.Fatal("killed helper unexpectedly exited cleanly")
	}
	waited = true

	lock, err := acquireConfigLock(path, time.Second)
	if err != nil {
		t.Fatalf("acquire after holder crash: %v", err)
	}
	if err := unlockConfigFile(lock); err != nil {
		t.Fatal(err)
	}
	if err := lock.Close(); err != nil {
		t.Fatal(err)
	}
}

func TestConfigWriteReplacesExistingFileWithoutPartialJSON(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	path, err := CLIConfigPath()
	if err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(`{"server_url":"old","future":true}`), 0o644); err != nil {
		t.Fatal(err)
	}
	cfg, err := LoadCLIConfig()
	if err != nil {
		t.Fatal(err)
	}
	cfg.ServerURL = "https://api.example.test"
	if err := SaveCLIConfig(cfg); err != nil {
		t.Fatal(err)
	}
	document := readTestConfigDocument(t, path)
	if document["server_url"] != "https://api.example.test" || document["future"] != true {
		t.Fatalf("unexpected replacement document: %#v", document)
	}
	if runtime.GOOS != "windows" && testFileMode(t, path) != 0o600 {
		t.Fatalf("config mode = %#o, want 0600", testFileMode(t, path))
	}
}
