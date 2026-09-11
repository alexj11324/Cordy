//go:build linux || freebsd || netbsd || openbsd || dragonfly

package daemon

import (
	"fmt"
	"os"
)

func platformMachineID() (string, error) {
	paths := []string{
		"/etc/machine-id",
		"/var/lib/dbus/machine-id",
		"/etc/hostid",
	}
	var firstErr error
	for _, path := range paths {
		data, err := os.ReadFile(path)
		if err != nil {
			if firstErr == nil {
				firstErr = err
			}
			continue
		}
		id := parseMachineIDContents(string(data))
		if id != "" {
			return id, nil
		}
	}
	if firstErr != nil {
		return "", fmt.Errorf("read OS machine-id: %w", firstErr)
	}
	return "", errEmptyMachineID
}
