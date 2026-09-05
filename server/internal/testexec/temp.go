// Package testexec provides executable fixture storage independently of /tmp,
// which can be a small noexec tmpfs on supported development hosts.
package testexec

import (
	"os"
	"path/filepath"
	"testing"
)

func TempDir(t testing.TB) string {
	t.Helper()
	cache := os.Getenv("XDG_CACHE_HOME")
	if cache == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			t.Fatal(err)
		}
		cache = filepath.Join(home, ".cache")
	}
	root := filepath.Join(cache, "codex-tmp-10g")
	if err := os.MkdirAll(root, 0700); err != nil {
		t.Fatal(err)
	}
	dir, err := os.MkdirTemp(root, "agent-fixture-")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := os.RemoveAll(dir); err != nil {
			t.Errorf("remove executable fixture: %v", err)
		}
	})
	return dir
}
