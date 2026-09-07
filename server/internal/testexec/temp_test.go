package testexec

import (
	"os"
	"path/filepath"
	"testing"
)

func TestRemoveAllWithRetryRemovesAnEmptyFixtureDir(t *testing.T) {
	root := t.TempDir()
	dir := filepath.Join(root, "agent-fixture-empty")
	if err := os.Mkdir(dir, 0700); err != nil {
		t.Fatal(err)
	}
	if err := removeAllWithRetry(dir); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(dir); !os.IsNotExist(err) {
		t.Fatalf("fixture dir still present: %v", err)
	}
}

func TestRemoveAllWithRetryTreatsAMissingDirAsSuccess(t *testing.T) {
	if err := removeAllWithRetry(filepath.Join(t.TempDir(), "already-gone")); err != nil {
		t.Fatal(err)
	}
}
