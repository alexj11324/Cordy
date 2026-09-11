//go:build !darwin && !windows && !linux && !freebsd && !netbsd && !openbsd && !dragonfly

package daemon

func platformMachineID() (string, error) {
	return "", errUnsupportedMachineID
}
