// Package testexec provides executable fixture storage independently of /tmp,
// which can be a small noexec tmpfs on supported development hosts.
package testexec

import (
	"os"
	"path/filepath"
	"testing"
	"time"
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
		if err := removeAllWithRetry(dir); err != nil {
			t.Errorf("remove executable fixture: %v", err)
		}
	})
	return dir
}

// removeAllWithRetry deletes dir even when Windows still holds a mapping to a
// fixture executable that just exited. processStillRunning can already be
// false while the image file remains undeletable for a short window.
func removeAllWithRetry(dir string) error {
	var err error
	delay := 20 * time.Millisecond
	for attempt := 0; attempt < 10; attempt++ {
		err = os.RemoveAll(dir)
		if err == nil {
			return nil
		}
		time.Sleep(delay)
		if delay < 400*time.Millisecond {
			delay *= 2
		}
	}
	return err
}
