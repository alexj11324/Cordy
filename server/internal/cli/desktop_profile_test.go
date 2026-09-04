package cli

import (
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

func TestDesktopProfileHelperPreservesUnknownFieldsAndKeepsCredentialsTogether(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv(TaskConfigRootEnv, "")
	profile := "desktop-api.example.test"
	path, err := CLIConfigPathForProfile(profile)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	initial := `{"workspace_id":"workspace-1","future":{"enabled":true},"token":"old","desktop_user_id":"old-user"}`
	if err := os.WriteFile(path, []byte(initial), 0o644); err != nil {
		t.Fatal(err)
	}

	request := `{"action":"set_credentials","profile":"` + profile + `","server_url":"https://api.example.test","token":"pby_secret","user_id":"user-1"}`
	if err := RunDesktopProfileHelper(strings.NewReader(request)); err != nil {
		t.Fatalf("set credentials: %v", err)
	}
	document := readTestConfigDocument(t, path)
	if got := document["server_url"]; got != "https://api.example.test" {
		t.Errorf("server_url = %#v", got)
	}
	if got := document["token"]; got != "pby_secret" {
		t.Errorf("token = %#v", got)
	}
	if got := document["desktop_user_id"]; got != "user-1" {
		t.Errorf("desktop_user_id = %#v", got)
	}
	if got := document["workspace_id"]; got != "workspace-1" {
		t.Errorf("workspace_id = %#v", got)
	}
	if future, ok := document["future"].(map[string]any); !ok || future["enabled"] != true {
		t.Errorf("future field lost: %#v", document["future"])
	}

	clear := `{"action":"clear_credentials","profile":"` + profile + `"}`
	if err := RunDesktopProfileHelper(strings.NewReader(clear)); err != nil {
		t.Fatalf("clear credentials: %v", err)
	}
	document = readTestConfigDocument(t, path)
	if _, ok := document["token"]; ok {
		t.Error("token survived clear")
	}
	if _, ok := document["desktop_user_id"]; ok {
		t.Error("desktop_user_id survived clear")
	}
	if document["workspace_id"] != "workspace-1" {
		t.Error("clear removed unrelated fields")
	}

	if runtime.GOOS != "windows" {
		if mode := testFileMode(t, path); mode != 0o600 {
			t.Errorf("config mode = %#o, want 0600", mode)
		}
		if mode := testFileMode(t, filepath.Dir(path)); mode != 0o700 {
			t.Errorf("profile directory mode = %#o, want 0700", mode)
		}
		if mode := testFileMode(t, filepath.Join(filepath.Dir(path), ".config.lock")); mode != 0o600 {
			t.Errorf("lock mode = %#o, want 0600", mode)
		}
	}
}

func TestDesktopProfileHelperRejectsUnsafeRequests(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv(TaskConfigRootEnv, "")
	for _, request := range []string{
		`{"action":"clear_credentials","profile":"default"}`,
		`{"action":"clear_credentials","profile":"desktop-../owner"}`,
		`{"action":"set_credentials","profile":"desktop-api","server_url":"https://api.example","token":"secret"}`,
		`{"action":"configure","profile":"desktop-api","server_url":""}`,
		`{"action":"unknown","profile":"desktop-api"}`,
		`{"action":"clear_credentials","profile":"desktop-api","future":true}`,
	} {
		if err := RunDesktopProfileHelper(strings.NewReader(request)); err == nil {
			t.Errorf("request unexpectedly accepted: %s", request)
		}
	}
}

func TestDesktopProfileHelperClearMissingIsIdempotent(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv(TaskConfigRootEnv, "")
	if err := RunDesktopProfileHelper(strings.NewReader(
		`{"action":"clear_credentials","profile":"desktop-api.example.test"}`,
	)); err != nil {
		t.Fatalf("clear missing profile: %v", err)
	}
}

func TestDesktopProfileHelperClearChecksExistenceUnderLock(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv(TaskConfigRootEnv, "")
	profile := "desktop-api.example.test"
	path, err := CLIConfigPathForProfile(profile)
	if err != nil {
		t.Fatal(err)
	}
	dir := filepath.Dir(path)
	if err := ensurePrivateProfileDirectory(dir); err != nil {
		t.Fatal(err)
	}
	lock, err := acquireConfigLock(filepath.Join(dir, ".config.lock"), time.Second)
	if err != nil {
		t.Fatal(err)
	}
	lockHeld := true
	defer func() {
		if lockHeld {
			_ = unlockConfigFile(lock)
			_ = lock.Close()
		}
	}()

	done := make(chan error, 1)
	go func() {
		done <- RunDesktopProfileHelper(strings.NewReader(
			`{"action":"clear_credentials","profile":"desktop-api.example.test"}`,
		))
	}()
	select {
	case err := <-done:
		t.Fatalf("clear returned before acquiring the config lock: %v", err)
	case <-time.After(75 * time.Millisecond):
	}

	if err := os.WriteFile(path, []byte(`{"token":"pby_new","desktop_user_id":"user-1","future":true}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := unlockConfigFile(lock); err != nil {
		t.Fatal(err)
	}
	if err := lock.Close(); err != nil {
		t.Fatal(err)
	}
	lockHeld = false
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	document := readTestConfigDocument(t, path)
	if _, ok := document["token"]; ok {
		t.Fatal("concurrent token survived clear")
	}
	if _, ok := document["desktop_user_id"]; ok {
		t.Fatal("concurrent Desktop user id survived clear")
	}
	if document["future"] != true {
		t.Fatal("clear lost an unrelated field")
	}
}

func TestDesktopProfileHelperRejectsTaskLocalConfigRoot(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv(TaskConfigRootEnv, filepath.Join(t.TempDir(), "task-root"))
	err := RunDesktopProfileHelper(strings.NewReader(
		`{"action":"configure","profile":"desktop-api.example.test","server_url":"https://api.example.test"}`,
	))
	if err == nil || !strings.Contains(err.Error(), "task-local config root") {
		t.Fatalf("helper error = %v, want task-root refusal", err)
	}
}

func readTestConfigDocument(t *testing.T, path string) map[string]any {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var document map[string]any
	if err := json.Unmarshal(data, &document); err != nil {
		t.Fatal(err)
	}
	return document
}

func testFileMode(t *testing.T, path string) os.FileMode {
	t.Helper()
	info, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	return info.Mode().Perm()
}
