package daemon

import (
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strings"
	"testing"

	"github.com/google/uuid"
)

func stubOSMachine(t *testing.T, machineID, username string) {
	t.Helper()
	origID, origUser := readOSMachineID, readOSUsername
	readOSMachineID = func() (string, error) { return machineID, nil }
	readOSUsername = func() string { return username }
	t.Cleanup(func() {
		readOSMachineID = origID
		readOSUsername = origUser
	})
}

func TestEnsureDaemonID_Persists(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	stubOSMachine(t, "11111111-2222-3333-4444-555555555555", "alex")
	want := DeriveDaemonID("11111111-2222-3333-4444-555555555555", "alex")

	first, superseded, err := EnsureDaemonID("")
	if err != nil {
		t.Fatalf("EnsureDaemonID first call: %v", err)
	}
	if first != want {
		t.Fatalf("EnsureDaemonID = %q, want OS-derived %q", first, want)
	}
	if len(superseded) != 0 {
		t.Fatalf("fresh install superseded = %v, want none", superseded)
	}

	path := filepath.Join(home, ".orvilo", "daemon.id")
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("daemon.id not written: %v", err)
	}
	if strings.TrimSpace(string(data)) != first {
		t.Fatalf("file contents %q differ from returned UUID %q", data, first)
	}

	second, _, err := EnsureDaemonID("")
	if err != nil {
		t.Fatalf("EnsureDaemonID second call: %v", err)
	}
	if second != first {
		t.Fatalf("UUID changed on second call: %q → %q", first, second)
	}
}

func TestEnsureDaemonID_SharedAcrossProfiles(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	stubOSMachine(t, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", "alex")

	defaultID, _, err := EnsureDaemonID("")
	if err != nil {
		t.Fatalf("default profile: %v", err)
	}
	stagingID, _, err := EnsureDaemonID("staging")
	if err != nil {
		t.Fatalf("staging profile: %v", err)
	}
	if defaultID != stagingID {
		t.Fatalf("profiles should share one machine id, got default=%s staging=%s", defaultID, stagingID)
	}

	profileFile := filepath.Join(home, ".orvilo", "profiles", "staging", "daemon.id")
	if _, err := os.Stat(profileFile); !os.IsNotExist(err) {
		t.Fatalf("profile-scoped daemon.id should not be created, stat err: %v", err)
	}
}

func TestEnsureDaemonID_MigratesPreChangeProfileFile(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	stubOSMachine(t, "deadbeef-0000-0000-0000-000000000001", "alex")
	want := DeriveDaemonID("deadbeef-0000-0000-0000-000000000001", "alex")

	legacyID := uuid.Must(uuid.NewV7()).String()
	profileDir := filepath.Join(home, ".orvilo", "profiles", "staging")
	if err := os.MkdirAll(profileDir, 0o755); err != nil {
		t.Fatalf("mkdir profile: %v", err)
	}
	if err := os.WriteFile(filepath.Join(profileDir, "daemon.id"), []byte(legacyID+"\n"), 0o600); err != nil {
		t.Fatalf("seed legacy id: %v", err)
	}

	got, superseded, err := EnsureDaemonID("staging")
	if err != nil {
		t.Fatalf("EnsureDaemonID: %v", err)
	}
	if got != want {
		t.Fatalf("expected OS-derived UUID %s, got %s", want, got)
	}
	if !containsString(superseded, legacyID) {
		t.Fatalf("superseded %v missing leftover profile UUID %s", superseded, legacyID)
	}

	data, err := os.ReadFile(filepath.Join(home, ".orvilo", "daemon.id"))
	if err != nil {
		t.Fatalf("read canonical file: %v", err)
	}
	if strings.TrimSpace(string(data)) != want {
		t.Fatalf("canonical file %q != derived %q", data, want)
	}
}

func TestEnsureDaemonID_MigratesRandomCachedUUID(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	stubOSMachine(t, "cafe0000-0000-0000-0000-000000000002", "alex")
	want := DeriveDaemonID("cafe0000-0000-0000-0000-000000000002", "alex")

	legacyID := uuid.Must(uuid.NewV7()).String()
	dir := filepath.Join(home, ".orvilo")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatalf("mkdir: %v", err)
	}
	if err := os.WriteFile(filepath.Join(dir, "daemon.id"), []byte(legacyID+"\n"), 0o600); err != nil {
		t.Fatalf("seed random id: %v", err)
	}

	got, superseded, err := EnsureDaemonID("")
	if err != nil {
		t.Fatalf("EnsureDaemonID: %v", err)
	}
	if got != want {
		t.Fatalf("got %s, want OS-derived %s", got, want)
	}
	if !containsString(superseded, legacyID) {
		t.Fatalf("superseded %v missing cached UUID %s", superseded, legacyID)
	}
}

func TestEnsureDaemonID_SurvivesDeletedCacheFile(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	stubOSMachine(t, "abcdabcd-abcd-abcd-abcd-abcdabcdabcd", "alex")

	first, _, err := EnsureDaemonID("")
	if err != nil {
		t.Fatalf("first: %v", err)
	}
	if err := os.Remove(filepath.Join(home, ".orvilo", "daemon.id")); err != nil {
		t.Fatalf("remove cache: %v", err)
	}
	second, superseded, err := EnsureDaemonID("")
	if err != nil {
		t.Fatalf("second: %v", err)
	}
	if second != first {
		t.Fatalf("reinstall minted a new id: %q → %q", first, second)
	}
	if len(superseded) != 0 {
		t.Fatalf("wiped cache should not report superseded, got %v", superseded)
	}
}

func TestEnsureDaemonID_RegeneratesCorruptFile(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	stubOSMachine(t, "99999999-0000-0000-0000-000000000003", "alex")
	want := DeriveDaemonID("99999999-0000-0000-0000-000000000003", "alex")

	dir := filepath.Join(home, ".orvilo")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatalf("mkdir: %v", err)
	}
	path := filepath.Join(dir, "daemon.id")
	if err := os.WriteFile(path, []byte("not-a-uuid"), 0o600); err != nil {
		t.Fatalf("seed corrupt file: %v", err)
	}

	id, superseded, err := EnsureDaemonID("")
	if err != nil {
		t.Fatalf("EnsureDaemonID: %v", err)
	}
	if id != want {
		t.Fatalf("got %q, want %q", id, want)
	}
	if len(superseded) != 0 {
		t.Fatalf("corrupt file is not a UUID to merge, superseded=%v", superseded)
	}

	data, _ := os.ReadFile(path)
	if strings.TrimSpace(string(data)) != id {
		t.Fatalf("file not rewritten with derived UUID")
	}
}

func TestEnsureDaemonID_FallsBackToCacheWhenOSUnavailable(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	legacyID := uuid.Must(uuid.NewV7()).String()
	dir := filepath.Join(home, ".orvilo")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatalf("mkdir: %v", err)
	}
	if err := os.WriteFile(filepath.Join(dir, "daemon.id"), []byte(legacyID+"\n"), 0o600); err != nil {
		t.Fatalf("seed cache: %v", err)
	}

	origID := readOSMachineID
	readOSMachineID = func() (string, error) { return "", errEmptyMachineID }
	t.Cleanup(func() { readOSMachineID = origID })

	got, superseded, err := EnsureDaemonID("")
	if err != nil {
		t.Fatalf("EnsureDaemonID: %v", err)
	}
	if got != legacyID {
		t.Fatalf("got %s, want cached %s", got, legacyID)
	}
	if len(superseded) != 0 {
		t.Fatalf("unavailable OS id should keep the cache, superseded=%v", superseded)
	}
}

func containsString(items []string, want string) bool {
	for _, item := range items {
		if item == want {
			return true
		}
	}
	return false
}

func TestLegacyDaemonUUIDs_ScansProfileDirs(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)

	uuidA := uuid.Must(uuid.NewV7()).String()
	uuidB := uuid.Must(uuid.NewV7()).String()
	for name, id := range map[string]string{"prod": uuidA, "desktop-orvilo": uuidB} {
		dir := filepath.Join(home, ".orvilo", "profiles", name)
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatalf("mkdir %s: %v", name, err)
		}
		if err := os.WriteFile(filepath.Join(dir, "daemon.id"), []byte(id+"\n"), 0o600); err != nil {
			t.Fatalf("write %s: %v", name, err)
		}
	}

	// A profile directory with a corrupt file must be skipped, not fail.
	corruptDir := filepath.Join(home, ".orvilo", "profiles", "corrupt")
	if err := os.MkdirAll(corruptDir, 0o755); err != nil {
		t.Fatalf("mkdir corrupt: %v", err)
	}
	if err := os.WriteFile(filepath.Join(corruptDir, "daemon.id"), []byte("not-a-uuid"), 0o600); err != nil {
		t.Fatalf("seed corrupt: %v", err)
	}

	got, err := LegacyDaemonUUIDs()
	if err != nil {
		t.Fatalf("LegacyDaemonUUIDs: %v", err)
	}
	sort.Strings(got)
	want := []string{uuidA, uuidB}
	sort.Strings(want)
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("LegacyDaemonUUIDs = %v, want %v", got, want)
	}
}

func TestLegacyDaemonUUIDs_MissingProfilesDirIsNil(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)

	ids, err := LegacyDaemonUUIDs()
	if err != nil {
		t.Fatalf("LegacyDaemonUUIDs: %v", err)
	}
	if ids != nil {
		t.Fatalf("expected nil on missing profiles dir, got %v", ids)
	}
}

func TestLegacyDaemonIDs(t *testing.T) {
	cases := []struct {
		name     string
		hostname string
		profile  string
		want     []string
	}{
		{
			name:     "plain hostname, no profile",
			hostname: "MacBook-Pro",
			want:     []string{"MacBook-Pro", "MacBook-Pro.local"},
		},
		{
			name:     "dot-local hostname, no profile",
			hostname: "MacBook-Pro.local",
			want:     []string{"MacBook-Pro", "MacBook-Pro.local"},
		},
		{
			name:     "plain hostname with profile",
			hostname: "MacBook-Pro",
			profile:  "staging",
			want: []string{
				"MacBook-Pro",
				"MacBook-Pro.local",
				"MacBook-Pro-staging",
				"MacBook-Pro.local-staging",
			},
		},
		{
			name:     "dot-local hostname with profile",
			hostname: "MacBook-Pro.local",
			profile:  "staging",
			want: []string{
				"MacBook-Pro",
				"MacBook-Pro.local",
				"MacBook-Pro-staging",
				"MacBook-Pro.local-staging",
			},
		},
		{
			name:     "empty hostname",
			hostname: "",
			want:     nil,
		},
		{
			name:     "mixed case hostname preserved as-is",
			hostname: "Jiayuans-MacBook-Pro.local",
			want: []string{
				"Jiayuans-MacBook-Pro",
				"Jiayuans-MacBook-Pro.local",
			},
		},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := LegacyDaemonIDs(tc.hostname, tc.profile)
			if !reflect.DeepEqual(got, tc.want) {
				t.Fatalf("LegacyDaemonIDs(%q, %q) = %v, want %v", tc.hostname, tc.profile, got, tc.want)
			}
		})
	}
}
