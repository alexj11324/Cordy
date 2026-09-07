//go:build windows

package terminalreport

import "golang.org/x/sys/windows"

// Windows commits report renames with MOVEFILE_WRITE_THROUGH below. A lost
// acknowledged-file deletion can only replay an already idempotent report.
func syncDirectory(string) error { return nil }

func durableRename(from, to string) error {
	source, err := windows.UTF16PtrFromString(from)
	if err != nil {
		return err
	}
	target, err := windows.UTF16PtrFromString(to)
	if err != nil {
		return err
	}
	return windows.MoveFileEx(source, target, windows.MOVEFILE_REPLACE_EXISTING|windows.MOVEFILE_WRITE_THROUGH)
}
