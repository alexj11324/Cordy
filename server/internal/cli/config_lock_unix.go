//go:build !windows

package cli

import (
	"os"

	"golang.org/x/sys/unix"
)

func openConfigLockFile(path string) (*os.File, error) {
	return os.OpenFile(path, os.O_CREATE|os.O_RDWR, 0o600)
}

func tryLockConfigFile(file *os.File) (bool, error) {
	err := unix.Flock(int(file.Fd()), unix.LOCK_EX|unix.LOCK_NB)
	if err == nil {
		return true, nil
	}
	if err == unix.EWOULDBLOCK || err == unix.EAGAIN {
		return false, nil
	}
	return false, err
}

func unlockConfigFile(file *os.File) error {
	return unix.Flock(int(file.Fd()), unix.LOCK_UN)
}

func restrictConfigFile(path string) error {
	return os.Chmod(path, 0o600)
}

func restrictConfigDirectory(path string) error {
	return os.Chmod(path, 0o700)
}

func replaceConfigFile(source, destination string) error {
	return os.Rename(source, destination)
}
