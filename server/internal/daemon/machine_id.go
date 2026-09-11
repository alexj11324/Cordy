package daemon

import (
	"os"
	"os/user"
	"strings"

	"github.com/google/uuid"
)

// daemonIDNamespace is the RFC 4122 name-based UUID namespace for Orvilo
// daemon identity. It is itself a UUID v5 of the DNS name "daemon.orvilo.ai"
// so the derived daemon_id stays stable across builds.
var daemonIDNamespace = uuid.NewSHA1(uuid.NameSpaceDNS, []byte("daemon.orvilo.ai"))

// Injected so tests can pin the OS machine-id without touching ioreg /
// the registry / /etc/machine-id.
var (
	readOSMachineID = platformMachineID
	readOSUsername  = platformUsername
)

// DeriveDaemonID returns the stable daemon UUID for one OS machine-id plus
// OS user. This is the industry default for "same computer" identity:
//
//   - macOS: IOPlatformUUID (IOKit)
//   - Windows: HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid
//   - Linux: /etc/machine-id (systemd) or /var/lib/dbus/machine-id
//
// The raw hardware/OS UUID is not used as daemon_id. UUID v5 over
// (lowercase machine-id, lowercase username) keeps the identifier
// deterministic across app reinstalls without sending the platform UUID
// to the server. Username is part of the name so two accounts on the
// same Mac do not share a daemon.
func DeriveDaemonID(machineID, username string) string {
	machineID = strings.ToLower(strings.TrimSpace(machineID))
	username = normalizeUsername(username)
	name := machineID + "\x1f" + username
	return uuid.NewSHA1(daemonIDNamespace, []byte(name)).String()
}

// normalizeUsername keeps one OS account stable across Windows DOMAIN\user,
// user@domain, and mixed-case variants. Two accounts on the same machine
// still hash to different daemon ids.
func normalizeUsername(username string) string {
	username = strings.ToLower(strings.TrimSpace(username))
	if i := strings.LastIndexAny(username, `\/`); i >= 0 {
		username = username[i+1:]
	}
	if i := strings.IndexByte(username, '@'); i >= 0 {
		username = username[:i]
	}
	username = strings.TrimSpace(username)
	if username == "" {
		return "default"
	}
	return username
}

func deriveHostDaemonID() (string, error) {
	machineID, err := readOSMachineID()
	if err != nil {
		return "", err
	}
	machineID = strings.TrimSpace(machineID)
	if machineID == "" {
		return "", errEmptyMachineID
	}
	return DeriveDaemonID(machineID, readOSUsername()), nil
}

func platformUsername() string {
	if u, err := user.Current(); err == nil {
		if name := strings.TrimSpace(u.Username); name != "" {
			return name
		}
	}
	if name := strings.TrimSpace(os.Getenv("USER")); name != "" {
		return name
	}
	if name := strings.TrimSpace(os.Getenv("USERNAME")); name != "" {
		return name
	}
	return "default"
}

func parseMachineIDContents(contents string) string {
	line := strings.TrimSpace(contents)
	if i := strings.IndexAny(line, "\r\n"); i >= 0 {
		line = strings.TrimSpace(line[:i])
	}
	return strings.ToLower(line)
}

func extractIOPlatformUUID(ioregOutput string) (string, error) {
	for _, line := range strings.Split(ioregOutput, "\n") {
		if !strings.Contains(line, "IOPlatformUUID") {
			continue
		}
		parts := strings.SplitAfter(line, `" = "`)
		if len(parts) != 2 {
			continue
		}
		id := strings.Trim(strings.TrimSpace(parts[1]), `"`)
		if id != "" {
			return id, nil
		}
	}
	return "", errMissingIOPlatformUUID
}
