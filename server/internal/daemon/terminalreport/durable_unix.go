//go:build !windows

package terminalreport

import (
	"os"
	"path/filepath"
)

func syncDirectory(path string) error {
	dir, err := os.Open(path)
	if err != nil {
		return err
	}
	defer dir.Close()
	return dir.Sync()
}

func durableRename(from, to string) error {
	if err := os.Rename(from, to); err != nil {
		return err
	}
	return syncDirectory(filepath.Dir(to))
}
