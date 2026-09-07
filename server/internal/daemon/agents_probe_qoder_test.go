package daemon

import (
	"testing"
)

// Qoder uses the same install-directory lookup as other providers when PATH
// does not contain its npm or native installer location.
func TestProbeAgentCLIs_QoderResolvesViaInstallPaths(t *testing.T) {
	orig := resolveAgentsFromInstallPaths
	t.Cleanup(func() { resolveAgentsFromInstallPaths = orig })
	resolveAgentsFromInstallPaths = func([]string) map[string]string {
		return map[string]string{"qodercli": "/fake/npm-global/bin/qodercli"}
	}

	// An empty PATH guarantees the exec.LookPath leg misses, so the only way
	// qoder can resolve is the install-path fallback.
	t.Setenv("PATH", "")

	agents := probeAgentCLIs()
	entry, ok := agents["qoder"]
	if !ok {
		t.Fatal("qoder was not discovered via the install-path fallback; " +
			"a GUI-launched daemon must discover conventional installations")
	}
	if entry.Path != "/fake/npm-global/bin/qodercli" {
		t.Errorf("qoder path = %q, want /fake/npm-global/bin/qodercli", entry.Path)
	}
	if entry.Command != "qodercli" {
		t.Errorf("qoder command = %q, want qodercli", entry.Command)
	}
}

func TestProbeAgentCLIs_QoderCNResolvesIndependently(t *testing.T) {
	orig := resolveAgentsFromInstallPaths
	t.Cleanup(func() { resolveAgentsFromInstallPaths = orig })
	resolveAgentsFromInstallPaths = func([]string) map[string]string {
		return map[string]string{
			"qodercli":   "/fake/bin/qodercli",
			"qoderclicn": "/fake/bin/qoderclicn",
		}
	}
	t.Setenv("PATH", "")

	agents := probeAgentCLIs()
	entry, ok := agents["qoderclicn"]
	if !ok {
		t.Fatal("qoderclicn was not discovered via the install-path fallback")
	}
	if entry.Path != "/fake/bin/qoderclicn" {
		t.Errorf("qoderclicn path = %q, want /fake/bin/qoderclicn", entry.Path)
	}
	if entry.Command != "qoderclicn" {
		t.Errorf("qoderclicn command = %q, want qoderclicn", entry.Command)
	}
	if _, ok := agents["qoder"]; !ok {
		t.Fatal("qodercli and qoderclicn must be registered independently when both are installed")
	}
}

// TestProbeAgentCLIs_QoderPinnedPathStaysHardMiss keeps the fallback from
// rescuing an operator-pinned ORVILO_QODER_PATH. A pinned absolute path that
// no longer exists must stay a miss rather than silently resolve a different
// binary — the same rule probe() applies to every other provider.
func TestProbeAgentCLIs_QoderPinnedPathStaysHardMiss(t *testing.T) {
	orig := resolveAgentsFromInstallPaths
	t.Cleanup(func() { resolveAgentsFromInstallPaths = orig })
	resolveAgentsFromInstallPaths = func([]string) map[string]string {
		return map[string]string{"qodercli": "/fake/npm-global/bin/qodercli"}
	}

	t.Setenv("PATH", "")
	t.Setenv("ORVILO_QODER_PATH", "/nonexistent/pinned/qodercli")

	if entry, ok := probeAgentCLIs()["qoder"]; ok {
		t.Errorf("pinned-but-missing ORVILO_QODER_PATH resolved to %q, want a hard miss", entry.Path)
	}
}

// Keep the audited command inventory aligned with both Qoder identities.
func TestDefaultAgentCommandNamesIncludesQoder(t *testing.T) {
	found := map[string]bool{}
	for _, name := range defaultAgentCommandNames {
		found[name] = true
	}
	for _, name := range []string{"qodercli", "qoderclicn"} {
		if !found[name] {
			t.Fatalf("defaultAgentCommandNames is missing %q; the command inventory "+
				"must include every built-in provider", name)
		}
	}
}
