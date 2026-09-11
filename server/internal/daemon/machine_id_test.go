package daemon

import (
	"strings"
	"testing"

	"github.com/google/uuid"
)

func TestDeriveDaemonID_StableForSameMachineAndUser(t *testing.T) {
	first := DeriveDaemonID("A95648D0-1234-5678-90AB-CDEF01234567", "Alex")
	second := DeriveDaemonID("a95648d0-1234-5678-90ab-cdef01234567", "alex")
	if first != second {
		t.Fatalf("case differences changed identity: %s vs %s", first, second)
	}
	if _, err := uuid.Parse(first); err != nil {
		t.Fatalf("derived id is not a UUID: %v", err)
	}
}

func TestDeriveDaemonID_DiffersByUserAndMachine(t *testing.T) {
	base := DeriveDaemonID("machine-a", "alex")
	otherUser := DeriveDaemonID("machine-a", "sam")
	otherMachine := DeriveDaemonID("machine-b", "alex")
	if base == otherUser {
		t.Fatal("two OS users on one machine must not share a daemon id")
	}
	if base == otherMachine {
		t.Fatal("two machines for one OS user must not share a daemon id")
	}
}

func TestDeriveDaemonID_NormalizesWindowsAndEmailUsernames(t *testing.T) {
	want := DeriveDaemonID("machine-a", "alex")
	for _, username := range []string{
		`CORP\Alex`,
		`corp/alex`,
		"alex@example.com",
		"  ALEX  ",
	} {
		got := DeriveDaemonID("machine-a", username)
		if got != want {
			t.Fatalf("username %q derived %s, want %s", username, got, want)
		}
	}
}

func TestExtractIOPlatformUUID(t *testing.T) {
	const ioreg = `+-o Mac <class IOPlatformExpertDevice, id 0x100000112, registered, matched, active, busy 0 (1 ms), retain 9>
    {
      "IOPlatformUUID" = "A95648D0-1234-5678-90AB-CDEF01234567"
    }
`
	got, err := extractIOPlatformUUID(ioreg)
	if err != nil {
		t.Fatalf("extractIOPlatformUUID: %v", err)
	}
	if got != "A95648D0-1234-5678-90AB-CDEF01234567" {
		t.Fatalf("got %q", got)
	}
}

func TestParseMachineIDContents(t *testing.T) {
	got := parseMachineIDContents("ABCDEF0123456789ABCDEF0123456789\n")
	if got != "abcdef0123456789abcdef0123456789" {
		t.Fatalf("got %q", got)
	}
}

func TestPlatformMachineID_AvailableOnThisHost(t *testing.T) {
	id, err := platformMachineID()
	if err != nil {
		t.Skipf("OS machine-id not readable: %v", err)
	}
	if strings.TrimSpace(id) == "" {
		t.Fatal("empty OS machine id")
	}
}
