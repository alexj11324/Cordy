//go:build darwin

package daemon

import (
	"bytes"
	"fmt"
	"io"
	"os/exec"
)

func platformMachineID() (string, error) {
	var stdout bytes.Buffer
	cmd := exec.Command("ioreg", "-rd1", "-c", "IOPlatformExpertDevice")
	cmd.Stdout = &stdout
	cmd.Stderr = io.Discard
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("ioreg: %w", err)
	}
	return extractIOPlatformUUID(stdout.String())
}
