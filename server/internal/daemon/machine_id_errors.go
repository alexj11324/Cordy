package daemon

import "errors"

var (
	errEmptyMachineID        = errors.New("empty OS machine id")
	errMissingIOPlatformUUID = errors.New("IOPlatformUUID not present in ioreg output")
	errUnsupportedMachineID  = errors.New("OS machine id is not available on this platform")
)
