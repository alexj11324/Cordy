//go:build windows

package daemon

import (
	"fmt"

	"golang.org/x/sys/windows/registry"
)

func platformMachineID() (string, error) {
	key, err := registry.OpenKey(
		registry.LOCAL_MACHINE,
		`SOFTWARE\Microsoft\Cryptography`,
		registry.QUERY_VALUE|registry.WOW64_64KEY,
	)
	if err != nil {
		return "", fmt.Errorf("open MachineGuid: %w", err)
	}
	defer key.Close()

	id, _, err := key.GetStringValue("MachineGuid")
	if err != nil {
		return "", fmt.Errorf("read MachineGuid: %w", err)
	}
	id = parseMachineIDContents(id)
	if id == "" {
		return "", errEmptyMachineID
	}
	return id, nil
}
