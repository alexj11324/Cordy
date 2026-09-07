package daemon

import (
	"bytes"
	"log/slog"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"runtime"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/orvilo-ai/orvilo/server/internal/cli"
	"github.com/orvilo-ai/orvilo/server/internal/daemon/execenv"
)

func TestResolveAgentExecutablePath_PreservesDispatchShimName(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("creating symlinks requires elevated privileges on Windows")
	}

	for _, shimName := range []string{"volta-shim", "vp"} {
		t.Run(shimName, func(t *testing.T) {
			managerDir := t.TempDir()
			manager := filepath.Join(managerDir, shimName)
			if err := os.WriteFile(manager, []byte("#!/bin/sh\nprintf '%s\\n' \"${0##*/}\"\n"), 0o755); err != nil {
				t.Fatalf("write dispatcher: %v", err)
			}

			binDir := t.TempDir()
			entrypoint := filepath.Join(binDir, "claude")
			if err := os.Symlink(manager, entrypoint); err != nil {
				t.Fatalf("symlink dispatcher: %v", err)
			}
			t.Setenv("PATH", binDir)

			got, err := resolveAgentExecutablePath("claude")
			if err != nil {
				t.Fatalf("resolveAgentExecutablePath: %v", err)
			}
			realBinDir, err := filepath.EvalSymlinks(binDir)
			if err != nil {
				t.Fatalf("resolve bin directory: %v", err)
			}
			want := filepath.Join(realBinDir, "claude")
			if got != want {
				t.Fatalf("resolved path = %q, want command-preserving entrypoint %q", got, want)
			}
			output, err := exec.Command(got, "--version").CombinedOutput()
			if err != nil {
				t.Fatalf("run resolved entrypoint: %v: %s", err, output)
			}
			if got := strings.TrimSpace(string(output)); got != "claude" {
				t.Fatalf("dispatcher observed command name %q, want claude", got)
			}
		})
	}
}

func TestResolveAgentExecutablePath_CanonicalizesOrdinaryVersionTarget(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("creating symlinks requires elevated privileges on Windows")
	}

	versionDir := t.TempDir()
	target := filepath.Join(versionDir, "claude-2.1.216")
	if err := os.WriteFile(target, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatalf("write versioned executable: %v", err)
	}

	binDir := t.TempDir()
	entrypoint := filepath.Join(binDir, "claude")
	if err := os.Symlink(target, entrypoint); err != nil {
		t.Fatalf("symlink versioned executable: %v", err)
	}
	t.Setenv("PATH", binDir)

	got, err := resolveAgentExecutablePath("claude")
	if err != nil {
		t.Fatalf("resolveAgentExecutablePath: %v", err)
	}
	want, err := filepath.EvalSymlinks(entrypoint)
	if err != nil {
		t.Fatalf("resolve versioned executable: %v", err)
	}
	if got != want {
		t.Fatalf("resolved path = %q, want pinned version target %q", got, want)
	}
}

func TestPatternsFromEnv_DefaultsWhenUnset(t *testing.T) {
	t.Setenv("ORVILO_GC_ARTIFACT_PATTERNS", "")
	defaults := []string{"node_modules", ".next", ".turbo"}
	got := patternsFromEnv("ORVILO_GC_ARTIFACT_PATTERNS", defaults)
	if !reflect.DeepEqual(got, defaults) {
		t.Fatalf("expected defaults %v, got %v", defaults, got)
	}
	// Ensure callers get a copy, not a shared backing array.
	got[0] = "mutated"
	if defaults[0] == "mutated" {
		t.Fatal("patternsFromEnv must not return a slice aliased with defaults")
	}
}

func TestDefaultGCIntervalIsTwoHours(t *testing.T) {
	if DefaultGCInterval != 2*time.Hour {
		t.Fatalf("DefaultGCInterval = %s, want 2h", DefaultGCInterval)
	}
}

// A localhost server URL is not the official cloud host, so this exercises the
// self-host branch of defaultGCCompletedTaskTTL: retention stays unbounded until
// an operator opts in, and a daemon upgrade never starts deleting on its own.
func TestLoadConfig_CompletedTaskTTLDefaultsDisabledOnSelfHostAndReadsEnv(t *testing.T) {
	stageFakeAgent(t)
	t.Setenv("HOME", t.TempDir())
	stubAgentInstallDirectories(t, nil)
	t.Setenv("ORVILO_GC_COMPLETED_TASK_TTL", "")

	overrides := Overrides{
		ServerURL:      "http://localhost:0",
		WorkspacesRoot: t.TempDir(),
	}
	cfg, err := LoadConfig(overrides)
	if err != nil {
		t.Fatalf("LoadConfig with default completed-task TTL: %v", err)
	}
	if cfg.GCCompletedTaskTTL != 0 {
		t.Fatalf("GCCompletedTaskTTL = %s, want disabled", cfg.GCCompletedTaskTTL)
	}

	t.Setenv("ORVILO_GC_COMPLETED_TASK_TTL", "36h")
	cfg, err = LoadConfig(overrides)
	if err != nil {
		t.Fatalf("LoadConfig with completed-task TTL: %v", err)
	}
	if cfg.GCCompletedTaskTTL != 36*time.Hour {
		t.Fatalf("GCCompletedTaskTTL = %s, want 36h", cfg.GCCompletedTaskTTL)
	}

	t.Setenv("ORVILO_GC_COMPLETED_TASK_TTL", "not-a-duration")
	if _, err := LoadConfig(overrides); err == nil || !strings.Contains(err.Error(), "ORVILO_GC_COMPLETED_TASK_TTL") {
		t.Fatalf("LoadConfig invalid completed-task TTL error = %v, want named validation error", err)
	}
}

func TestLoadConfig_CompletedTaskTTLDefaultsBoundedOnOfficialCloud(t *testing.T) {
	stageFakeAgent(t)
	t.Setenv("HOME", t.TempDir())
	stubAgentInstallDirectories(t, nil)
	t.Setenv("ORVILO_GC_COMPLETED_TASK_TTL", "")

	overrides := Overrides{
		ServerURL:      "https://" + officialCloudHost,
		WorkspacesRoot: t.TempDir(),
	}
	cfg, err := LoadConfig(overrides)
	if err != nil {
		t.Fatalf("LoadConfig on official cloud: %v", err)
	}
	if cfg.GCCompletedTaskTTL != 14*24*time.Hour {
		t.Fatalf("GCCompletedTaskTTL = %s, want 14d on official cloud", cfg.GCCompletedTaskTTL)
	}

	// An explicit 0 has to win on cloud too — otherwise the only way back to the
	// previous retention behavior would be downgrading the daemon.
	t.Setenv("ORVILO_GC_COMPLETED_TASK_TTL", "0")
	cfg, err = LoadConfig(overrides)
	if err != nil {
		t.Fatalf("LoadConfig with cloud opt-out: %v", err)
	}
	if cfg.GCCompletedTaskTTL != 0 {
		t.Fatalf("GCCompletedTaskTTL = %s, want an explicit 0 to disable the cloud default", cfg.GCCompletedTaskTTL)
	}

	t.Setenv("ORVILO_GC_COMPLETED_TASK_TTL", "36h")
	cfg, err = LoadConfig(overrides)
	if err != nil {
		t.Fatalf("LoadConfig with cloud override: %v", err)
	}
	if cfg.GCCompletedTaskTTL != 36*time.Hour {
		t.Fatalf("GCCompletedTaskTTL = %s, want the env override to win on cloud", cfg.GCCompletedTaskTTL)
	}
}

func TestDefaultGCCompletedTaskTTLOnlyBoundsOfficialCloudHost(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name      string
		serverURL string
		want      time.Duration
	}{
		{"official cloud", "https://api.aspectlylabs.com", DefaultGCCompletedTaskTTLCloud},
		{"official cloud with port and path", "https://API.AspectlyLabs.Com:443/api", DefaultGCCompletedTaskTTLCloud},
		// Staging and previews inherit the self-host value for the same reason
		// officialCloudHost excludes them from the auto-update default.
		{"staging", "https://api-staging.aspectlylabs.com", DefaultGCCompletedTaskTTLSelfHost},
		{"self-host", "https://orvilo.example.com", DefaultGCCompletedTaskTTLSelfHost},
		{"localhost", "http://localhost:8080", DefaultGCCompletedTaskTTLSelfHost},
		{"unparseable", "://nope", DefaultGCCompletedTaskTTLSelfHost},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			if got := defaultGCCompletedTaskTTL(tc.serverURL); got != tc.want {
				t.Fatalf("defaultGCCompletedTaskTTL(%q) = %s, want %s", tc.serverURL, got, tc.want)
			}
		})
	}
}

func TestRepoMaintenanceKillSwitchDefaultsOnAndCanDisable(t *testing.T) {
	t.Setenv("ORVILO_GC_REPO_MAINTENANCE_ENABLED", "")
	if !boolFromEnv("ORVILO_GC_REPO_MAINTENANCE_ENABLED", true) {
		t.Fatal("repo maintenance kill switch should default to enabled")
	}

	t.Setenv("ORVILO_GC_REPO_MAINTENANCE_ENABLED", "false")
	if boolFromEnv("ORVILO_GC_REPO_MAINTENANCE_ENABLED", true) {
		t.Fatal("repo maintenance kill switch should accept false")
	}
}

func TestPatternsFromEnv_DropsSeparatorBearingEntries(t *testing.T) {
	t.Setenv("ORVILO_GC_ARTIFACT_PATTERNS", "node_modules, .next ,foo/bar, ../etc, ,target")
	got := patternsFromEnv("ORVILO_GC_ARTIFACT_PATTERNS", nil)
	want := []string{"node_modules", ".next", "target"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("expected %v, got %v", want, got)
	}
}

func TestIsSafeAgentName(t *testing.T) {
	for _, tc := range []struct {
		in   string
		want bool
	}{
		{"claude", true},
		{"cursor-agent", true},
		{"kiro_cli", true},
		{"v1.2", true},
		{"Claude2", true},
		{"", false},
		{"a b", false},
		{"a/b", false},
		{"a;b", false},
		{"a$b", false},
		{"a`b", false},
		{"a'b", false},
		{`a"b`, false},
	} {
		if got := isSafeAgentName(tc.in); got != tc.want {
			t.Errorf("isSafeAgentName(%q) = %v, want %v", tc.in, got, tc.want)
		}
	}
}

func TestIsOfficialCloudServer(t *testing.T) {
	for _, tc := range []struct {
		name string
		url  string
		want bool
	}{
		{"canonical cloud https", "https://api.aspectlylabs.com", true},
		{"canonical cloud with trailing slash stripped", "https://api.aspectlylabs.com/", true},
		{"canonical cloud case-insensitive", "https://API.AspectlyLabs.Com", true},
		{"cloud over plain http (unusual but match host)", "http://api.aspectlylabs.com", true},
		{"localhost is self-host", "http://localhost:8080", false},
		{"loopback ip is self-host", "http://127.0.0.1:8080", false},
		{"lan ip is self-host", "http://192.168.0.28:8080", false},
		{"third-party host is self-host", "https://orvilo.example.com", false},
		// Staging / preview / future subdomains deliberately follow the
		// safer self-host default until explicitly opted in.
		{"aspectlylabs.com apex is not the api host", "https://aspectlylabs.com", false},
		{"staging subdomain is self-host", "https://staging.aspectlylabs.com", false},
		{"preview subdomain is self-host", "https://api-preview.aspectlylabs.com", false},
		// Malformed inputs must not falsely match.
		{"empty string is self-host", "", false},
		{"garbage string is self-host", "::not a url::", false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := isOfficialCloudServer(tc.url); got != tc.want {
				t.Errorf("isOfficialCloudServer(%q) = %v, want %v", tc.url, got, tc.want)
			}
		})
	}
}

// stageFakeAgent writes an executable `claude` script into a temp dir and
// points PATH (and the daemon-id env var) so LoadConfig can run end-to-end
// without poking the host's real agent installation. Returns the staged PATH
// so tests that need to add their own dirs can extend it.
func stageFakeAgent(t *testing.T) string {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("POSIX shell not available on Windows")
	}
	binDir := t.TempDir()
	fake := filepath.Join(binDir, "claude")
	if err := os.WriteFile(fake, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatalf("write fake claude: %v", err)
	}
	t.Setenv("PATH", binDir)
	t.Setenv("ORVILO_DAEMON_ID", "11111111-1111-1111-1111-111111111111")
	// Clear any inherited env-var override so the test sees the URL-based
	// default, not whatever the developer happens to have exported.
	t.Setenv("ORVILO_DAEMON_AUTO_UPDATE", "")
	return binDir
}

func TestLoadConfig_DiscoversQwenCode(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("POSIX shell fixture is unavailable on Windows")
	}
	binDir := stageFakeAgent(t)
	qwen := filepath.Join(binDir, "qwen")
	if err := os.WriteFile(qwen, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatalf("write fake qwen: %v", err)
	}
	// Exclude host installations; this fixture exercises ordinary PATH discovery.
	stubAgentInstallDirectories(t, nil)
	t.Setenv("ORVILO_QWEN_MODEL", "qwen3.8-max-preview")
	t.Setenv("ORVILO_QWEN_ARGS", "--verbose --foo=bar")

	cfg, err := LoadConfig(Overrides{
		ServerURL:      "http://localhost:0",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig: %v", err)
	}
	entry, ok := cfg.Agents["qwen"]
	if !ok {
		t.Fatalf("qwen was not discovered: %v", cfg.Agents)
	}
	wantPath, err := filepath.EvalSymlinks(qwen)
	if err != nil {
		t.Fatalf("eval symlinks for qwen: %v", err)
	}
	if entry.Path != wantPath || entry.Command != "qwen" || entry.Model != "qwen3.8-max-preview" {
		t.Fatalf("qwen entry = %+v, want path=%q command=qwen model=qwen3.8-max-preview", entry, wantPath)
	}
	if got, want := strings.Join(cfg.QwenArgs, " "), "--verbose --foo=bar"; got != want {
		t.Fatalf("QwenArgs = %q, want %q", got, want)
	}
}

func TestLoadConfig_SkipsOrviloHooksShadowingAgentBinaries(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("POSIX shell not available on Windows")
	}

	home := t.TempDir()
	t.Setenv("HOME", home)
	hooksDir := filepath.Join(home, ".orvilo", "hooks")
	if err := os.MkdirAll(hooksDir, 0o755); err != nil {
		t.Fatalf("create hooks dir: %v", err)
	}
	realBinDir := t.TempDir()

	for _, name := range []string{"claude", "codex", "hermes"} {
		hookPath := filepath.Join(hooksDir, name)
		hookBody := "#!/bin/sh\nexec " + name + " \"$@\"\n"
		if err := os.WriteFile(hookPath, []byte(hookBody), 0o755); err != nil {
			t.Fatalf("write hook wrapper %s: %v", name, err)
		}
		realPath := filepath.Join(realBinDir, name)
		if err := os.WriteFile(realPath, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
			t.Fatalf("write real binary %s: %v", name, err)
		}
	}

	t.Setenv("PATH", hooksDir+string(os.PathListSeparator)+realBinDir)
	stubAgentInstallDirectories(t, nil)
	t.Setenv("ORVILO_DAEMON_ID", "11111111-1111-1111-1111-111111111111")

	cfg, err := LoadConfig(Overrides{
		ServerURL:      "http://localhost:0",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig: %v", err)
	}

	for provider, binary := range map[string]string{
		"claude": "claude",
		"codex":  "codex",
		"hermes": "hermes",
	} {
		got, ok := cfg.Agents[provider]
		if !ok {
			t.Fatalf("expected %s agent in config, got %#v", provider, cfg.Agents)
		}
		want := canonicalExecutablePath(filepath.Join(realBinDir, binary))
		if got.Path != want {
			t.Errorf("%s path = %q, want unshadowed real binary %q", provider, got.Path, want)
		}
		if strings.HasPrefix(got.Path, hooksDir) {
			t.Errorf("%s path still points into hooks dir: %q", provider, got.Path)
		}
	}
}

func TestLoadConfig_AutoUpdateDefault_SelfHostOff(t *testing.T) {
	stageFakeAgent(t)
	cfg, err := LoadConfig(Overrides{
		ServerURL:      "http://localhost:8080",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig: %v", err)
	}
	if cfg.AutoUpdateEnabled {
		t.Fatalf("AutoUpdateEnabled = true for self-host (localhost) server, want false")
	}
}

func TestLoadConfig_CodexHandshakeTimeout(t *testing.T) {
	stageFakeAgent(t)
	t.Setenv("ORVILO_CODEX_HANDSHAKE_TIMEOUT", "")

	cfg, err := LoadConfig(Overrides{
		ServerURL:      "http://localhost:8080",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig with default: %v", err)
	}
	if cfg.CodexHandshakeTimeout != DefaultCodexHandshakeTimeout {
		t.Fatalf("CodexHandshakeTimeout = %s, want default %s", cfg.CodexHandshakeTimeout, DefaultCodexHandshakeTimeout)
	}
	if cfg.CodexThreadHandshakeTimeout != DefaultCodexThreadHandshakeTimeout {
		t.Fatalf("CodexThreadHandshakeTimeout = %s, want default %s", cfg.CodexThreadHandshakeTimeout, DefaultCodexThreadHandshakeTimeout)
	}

	t.Setenv("ORVILO_CODEX_HANDSHAKE_TIMEOUT", "47s")

	cfg, err = LoadConfig(Overrides{
		ServerURL:      "http://localhost:8080",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig with env: %v", err)
	}
	if cfg.CodexHandshakeTimeout != 47*time.Second {
		t.Fatalf("CodexHandshakeTimeout = %s, want 47s from env", cfg.CodexHandshakeTimeout)
	}
	if cfg.CodexThreadHandshakeTimeout != 47*time.Second {
		t.Fatalf("CodexThreadHandshakeTimeout = %s, want legacy 47s env override", cfg.CodexThreadHandshakeTimeout)
	}

	t.Setenv("ORVILO_CODEX_HANDSHAKE_TIMEOUT", "1d")
	cfg, err = LoadConfig(Overrides{
		ServerURL:      "http://localhost:8080",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig with day-unit env: %v", err)
	}
	if cfg.CodexHandshakeTimeout != 24*time.Hour {
		t.Fatalf("CodexHandshakeTimeout = %s, want 24h from 1d env", cfg.CodexHandshakeTimeout)
	}
	if cfg.CodexThreadHandshakeTimeout != 24*time.Hour {
		t.Fatalf("CodexThreadHandshakeTimeout = %s, want legacy 24h env override", cfg.CodexThreadHandshakeTimeout)
	}

	t.Setenv("ORVILO_CODEX_HANDSHAKE_TIMEOUT", "0")
	cfg, err = LoadConfig(Overrides{
		ServerURL:      "http://localhost:8080",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig with zero env: %v", err)
	}
	if cfg.CodexHandshakeTimeout != DefaultCodexHandshakeTimeout {
		t.Fatalf("CodexHandshakeTimeout = %s, want default %s for zero env", cfg.CodexHandshakeTimeout, DefaultCodexHandshakeTimeout)
	}
	if cfg.CodexThreadHandshakeTimeout != DefaultCodexThreadHandshakeTimeout {
		t.Fatalf("CodexThreadHandshakeTimeout = %s, want default %s for zero env", cfg.CodexThreadHandshakeTimeout, DefaultCodexThreadHandshakeTimeout)
	}

	cfg, err = LoadConfig(Overrides{
		ServerURL:             "http://localhost:8080",
		WorkspacesRoot:        t.TempDir(),
		CodexHandshakeTimeout: 12 * time.Second,
	})
	if err != nil {
		t.Fatalf("LoadConfig with override: %v", err)
	}
	if cfg.CodexHandshakeTimeout != 12*time.Second {
		t.Fatalf("CodexHandshakeTimeout = %s, want 12s from override", cfg.CodexHandshakeTimeout)
	}
	if cfg.CodexThreadHandshakeTimeout != 12*time.Second {
		t.Fatalf("CodexThreadHandshakeTimeout = %s, want legacy 12s override", cfg.CodexThreadHandshakeTimeout)
	}
}

// TestLoadConfig_CodexFirstTurnNoProgressTimeout pins the env-only
// ORVILO_CODEX_FIRST_TURN_TIMEOUT resolution (GH #3262 / #5959): unset and an
// explicit "0" both mean "keep the backend default" (0 = unset), while a positive
// value is honored verbatim. There is deliberately no Overrides/CLI parity — this
// knob is environment-only.
func TestLoadConfig_CodexFirstTurnNoProgressTimeout(t *testing.T) {
	stageFakeAgent(t)
	t.Setenv("ORVILO_CODEX_FIRST_TURN_TIMEOUT", "")

	cfg, err := LoadConfig(Overrides{
		ServerURL:      "http://localhost:8080",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig with unset: %v", err)
	}
	if cfg.CodexFirstTurnNoProgressTimeout != 0 {
		t.Fatalf("CodexFirstTurnNoProgressTimeout = %s, want 0 when unset", cfg.CodexFirstTurnNoProgressTimeout)
	}

	t.Setenv("ORVILO_CODEX_FIRST_TURN_TIMEOUT", "30m")
	cfg, err = LoadConfig(Overrides{
		ServerURL:      "http://localhost:8080",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig with env: %v", err)
	}
	if cfg.CodexFirstTurnNoProgressTimeout != 30*time.Minute {
		t.Fatalf("CodexFirstTurnNoProgressTimeout = %s, want 30m from env", cfg.CodexFirstTurnNoProgressTimeout)
	}

	t.Setenv("ORVILO_CODEX_FIRST_TURN_TIMEOUT", "0")
	cfg, err = LoadConfig(Overrides{
		ServerURL:      "http://localhost:8080",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig with zero env: %v", err)
	}
	if cfg.CodexFirstTurnNoProgressTimeout != 0 {
		t.Fatalf("CodexFirstTurnNoProgressTimeout = %s, want 0 for explicit zero", cfg.CodexFirstTurnNoProgressTimeout)
	}
}

// TestLoadConfig_CodexFirstTurnTimeoutEqualToSemanticWarns pins the equality
// edge (orvilo-eve review on #6753): because the semantic-inactivity timer is
// armed before the first-turn timer, equal durations still let the semantic
// deadline win and drop the #3291 startup retry. LoadConfig must warn when the
// first-turn timeout is >= the semantic timeout, and stay quiet only when the
// semantic timeout is strictly greater.
func TestLoadConfig_CodexFirstTurnTimeoutEqualToSemanticWarns(t *testing.T) {
	stageFakeAgent(t)

	const warnNeedle = "ORVILO_CODEX_FIRST_TURN_TIMEOUT is greater than or equal to the semantic-inactivity timeout"

	loadWithLoggedWarnings := func(t *testing.T, semantic, firstTurn string) string {
		t.Helper()
		var buf bytes.Buffer
		prev := slog.Default()
		slog.SetDefault(slog.New(slog.NewTextHandler(&buf, &slog.HandlerOptions{Level: slog.LevelWarn})))
		t.Cleanup(func() { slog.SetDefault(prev) })

		t.Setenv("ORVILO_CODEX_SEMANTIC_INACTIVITY_TIMEOUT", semantic)
		t.Setenv("ORVILO_CODEX_FIRST_TURN_TIMEOUT", firstTurn)
		if _, err := LoadConfig(Overrides{
			ServerURL:      "http://localhost:8080",
			WorkspacesRoot: t.TempDir(),
		}); err != nil {
			t.Fatalf("LoadConfig: %v", err)
		}
		return buf.String()
	}

	// Equal durations: the semantic timer is armed first, so the retry can still
	// be lost. The warning MUST fire — this is the edge the earlier `>`-only
	// check missed.
	if logs := loadWithLoggedWarnings(t, "15m", "15m"); !strings.Contains(logs, warnNeedle) {
		t.Fatalf("equal durations must warn; logs = %q", logs)
	}

	// First-turn strictly above semantic: also warns (the original truncation case).
	if logs := loadWithLoggedWarnings(t, "10m", "30m"); !strings.Contains(logs, warnNeedle) {
		t.Fatalf("first-turn above semantic must warn; logs = %q", logs)
	}

	// Semantic strictly above first-turn: the recommended safe configuration —
	// no warning.
	if logs := loadWithLoggedWarnings(t, "30m", "10m"); strings.Contains(logs, warnNeedle) {
		t.Fatalf("semantic strictly above first-turn must not warn; logs = %q", logs)
	}
}

// TestLoadConfig_ToolWatchdogDefaultsToIdleWatchdog pins the merge: the
// in-flight-tool budget is no longer an independent constant, so an operator
// who raises the idle budget cannot accidentally leave tool calls capped at a
// lower, invisible ceiling.
func TestLoadConfig_ToolWatchdogDefaultsToIdleWatchdog(t *testing.T) {
	stageFakeAgent(t)
	t.Setenv("ORVILO_AGENT_IDLE_WATCHDOG", "")
	t.Setenv("ORVILO_AGENT_TOOL_WATCHDOG", "")

	load := func(t *testing.T) Config {
		t.Helper()
		cfg, err := LoadConfig(Overrides{
			ServerURL:      "http://localhost:8080",
			WorkspacesRoot: t.TempDir(),
		})
		if err != nil {
			t.Fatalf("LoadConfig: %v", err)
		}
		return cfg
	}

	cfg := load(t)
	if cfg.AgentIdleWatchdog != DefaultAgentIdleWatchdog {
		t.Fatalf("AgentIdleWatchdog = %s, want default %s", cfg.AgentIdleWatchdog, DefaultAgentIdleWatchdog)
	}
	if cfg.AgentToolWatchdog != cfg.AgentIdleWatchdog {
		t.Fatalf("AgentToolWatchdog = %s, want it to track AgentIdleWatchdog %s", cfg.AgentToolWatchdog, cfg.AgentIdleWatchdog)
	}

	// Raising only the idle budget must carry the tool budget with it.
	t.Setenv("ORVILO_AGENT_IDLE_WATCHDOG", "6h")
	cfg = load(t)
	if cfg.AgentIdleWatchdog != 6*time.Hour || cfg.AgentToolWatchdog != 6*time.Hour {
		t.Fatalf("idle=%s tool=%s, want both 6h", cfg.AgentIdleWatchdog, cfg.AgentToolWatchdog)
	}

	// An explicit tool override still wins, so "tools may run longer than the
	// model may think" stays expressible.
	t.Setenv("ORVILO_AGENT_TOOL_WATCHDOG", "12h")
	cfg = load(t)
	if cfg.AgentIdleWatchdog != 6*time.Hour {
		t.Fatalf("AgentIdleWatchdog = %s, want 6h", cfg.AgentIdleWatchdog)
	}
	if cfg.AgentToolWatchdog != 12*time.Hour {
		t.Fatalf("AgentToolWatchdog = %s, want 12h from env", cfg.AgentToolWatchdog)
	}

	// Zero keeps its distinct meaning: never force-stop while a tool is in
	// flight. It must NOT be re-derived from the idle budget.
	t.Setenv("ORVILO_AGENT_TOOL_WATCHDOG", "0")
	cfg = load(t)
	if cfg.AgentToolWatchdog != 0 {
		t.Fatalf("AgentToolWatchdog = %s, want 0 from env", cfg.AgentToolWatchdog)
	}

	// Disabling the suite disables both.
	t.Setenv("ORVILO_AGENT_IDLE_WATCHDOG", "0")
	t.Setenv("ORVILO_AGENT_TOOL_WATCHDOG", "")
	cfg = load(t)
	if cfg.AgentIdleWatchdog != 0 || cfg.AgentToolWatchdog != 0 {
		t.Fatalf("idle=%s tool=%s, want both 0", cfg.AgentIdleWatchdog, cfg.AgentToolWatchdog)
	}
}

// TestLoadConfig_CodexSemanticInactivityDerivesFromWatchdog pins the fix for
// the gap the 2h change left behind: Codex's own semantic-inactivity timer is
// not tool-aware, so if it keeps a 10m ceiling it kills a quiet build long
// before the daemon budget that was supposed to protect it, and the raised
// budget is a lie for Codex users.
func TestLoadConfig_CodexSemanticInactivityDerivesFromWatchdog(t *testing.T) {
	stageFakeAgent(t)
	for _, key := range []string{
		"ORVILO_AGENT_IDLE_WATCHDOG",
		"ORVILO_AGENT_TOOL_WATCHDOG",
		"ORVILO_CODEX_SEMANTIC_INACTIVITY_TIMEOUT",
	} {
		t.Setenv(key, "")
	}

	load := func(t *testing.T) Config {
		t.Helper()
		cfg, err := LoadConfig(Overrides{
			ServerURL:      "http://localhost:8080",
			WorkspacesRoot: t.TempDir(),
		})
		if err != nil {
			t.Fatalf("LoadConfig: %v", err)
		}
		return cfg
	}

	cfg := load(t)
	if cfg.CodexSemanticInactivityTimeout != DefaultAgentIdleWatchdog {
		t.Fatalf("CodexSemanticInactivityTimeout = %s, want the idle budget %s", cfg.CodexSemanticInactivityTimeout, DefaultAgentIdleWatchdog)
	}

	// Raising the idle budget carries Codex with it.
	t.Setenv("ORVILO_AGENT_IDLE_WATCHDOG", "6h")
	if got := load(t).CodexSemanticInactivityTimeout; got != 6*time.Hour {
		t.Fatalf("CodexSemanticInactivityTimeout = %s, want 6h", got)
	}

	// A wider tool budget wins: this timer cannot see that a tool is in flight,
	// so it has to be sized like the larger of the two or it re-creates the very
	// bug being fixed, one tier up.
	t.Setenv("ORVILO_AGENT_TOOL_WATCHDOG", "9h")
	if got := load(t).CodexSemanticInactivityTimeout; got != 9*time.Hour {
		t.Fatalf("CodexSemanticInactivityTimeout = %s, want the wider tool budget 9h", got)
	}

	// A tool budget of 0 means "never force-stop during a tool", which this
	// timer cannot express; it falls back to the idle budget rather than
	// silently running unbounded.
	t.Setenv("ORVILO_AGENT_TOOL_WATCHDOG", "0")
	if got := load(t).CodexSemanticInactivityTimeout; got != 6*time.Hour {
		t.Fatalf("CodexSemanticInactivityTimeout = %s, want the idle budget 6h when the tool budget is unbounded", got)
	}

	// The explicit env still wins over the derivation.
	t.Setenv("ORVILO_CODEX_SEMANTIC_INACTIVITY_TIMEOUT", "20m")
	if got := load(t).CodexSemanticInactivityTimeout; got != 20*time.Minute {
		t.Fatalf("CodexSemanticInactivityTimeout = %s, want 20m from env", got)
	}

	// Disabling the watchdog suite has never disabled this timer; Codex keeps
	// its own built-in default rather than becoming unbounded.
	t.Setenv("ORVILO_CODEX_SEMANTIC_INACTIVITY_TIMEOUT", "")
	t.Setenv("ORVILO_AGENT_IDLE_WATCHDOG", "0")
	t.Setenv("ORVILO_AGENT_TOOL_WATCHDOG", "")
	if got := load(t).CodexSemanticInactivityTimeout; got != DefaultCodexSemanticInactivityTimeout {
		t.Fatalf("CodexSemanticInactivityTimeout = %s, want the codex built-in %s when watchdogs are off", got, DefaultCodexSemanticInactivityTimeout)
	}
}

func TestLoadConfig_OpenCodeIdleWatchdog(t *testing.T) {
	stageFakeAgent(t)
	t.Setenv("ORVILO_OPENCODE_IDLE_WATCHDOG", "")

	cfg, err := LoadConfig(Overrides{
		ServerURL:      "http://localhost:8080",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig with default: %v", err)
	}
	if cfg.OpenCodeIdleWatchdog != DefaultOpenCodeIdleWatchdog {
		t.Fatalf("OpenCodeIdleWatchdog = %s, want default %s", cfg.OpenCodeIdleWatchdog, DefaultOpenCodeIdleWatchdog)
	}

	t.Setenv("ORVILO_OPENCODE_IDLE_WATCHDOG", "7m")
	cfg, err = LoadConfig(Overrides{
		ServerURL:      "http://localhost:8080",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig with env: %v", err)
	}
	if cfg.OpenCodeIdleWatchdog != 7*time.Minute {
		t.Fatalf("OpenCodeIdleWatchdog = %s, want 7m from env", cfg.OpenCodeIdleWatchdog)
	}

	// Zero disables the OpenCode-specific override while leaving the generic
	// AgentIdleWatchdog as the fallback for OpenCode runs.
	t.Setenv("ORVILO_OPENCODE_IDLE_WATCHDOG", "0")
	cfg, err = LoadConfig(Overrides{
		ServerURL:      "http://localhost:8080",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig with zero env: %v", err)
	}
	if cfg.OpenCodeIdleWatchdog != 0 {
		t.Fatalf("OpenCodeIdleWatchdog = %s, want zero from env", cfg.OpenCodeIdleWatchdog)
	}
}

// TestLoadConfig_AutoUpdateDefault_CloudOn confirms the symmetric case: a
// daemon pointed at Orvilo's hosted cloud keeps the historical opt-in
// auto-update default. We pass the WSS form of the URL to also exercise that
// NormalizeServerBaseURL maps it through to the http host the detector
// inspects.
func TestLoadConfig_AutoUpdateDefault_CloudOn(t *testing.T) {
	stageFakeAgent(t)
	cfg, err := LoadConfig(Overrides{
		ServerURL:      "wss://api.aspectlylabs.com/ws",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig: %v", err)
	}
	if !cfg.AutoUpdateEnabled {
		t.Fatalf("AutoUpdateEnabled = false for Orvilo Cloud server, want true")
	}
}

// TestLoadConfig_AutoUpdateEnv_ForcesOnForSelfHost lets a self-host operator
// re-enable auto-update via env var, overriding the new conservative default.
func TestLoadConfig_AutoUpdateEnv_ForcesOnForSelfHost(t *testing.T) {
	stageFakeAgent(t)
	t.Setenv("ORVILO_DAEMON_AUTO_UPDATE", "true")
	cfg, err := LoadConfig(Overrides{
		ServerURL:      "http://localhost:8080",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig: %v", err)
	}
	if !cfg.AutoUpdateEnabled {
		t.Fatalf("AutoUpdateEnabled = false after explicit ORVILO_DAEMON_AUTO_UPDATE=true, want true")
	}
}

// TestLoadConfig_AutoUpdateEnv_ForcesOffForCloud covers the inverse: a cloud
// user can still opt out via env var.
func TestLoadConfig_AutoUpdateEnv_ForcesOffForCloud(t *testing.T) {
	stageFakeAgent(t)
	t.Setenv("ORVILO_DAEMON_AUTO_UPDATE", "false")
	cfg, err := LoadConfig(Overrides{
		ServerURL:      "https://api.aspectlylabs.com",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig: %v", err)
	}
	if cfg.AutoUpdateEnabled {
		t.Fatalf("AutoUpdateEnabled = true after explicit ORVILO_DAEMON_AUTO_UPDATE=false, want false")
	}
}

// TestLoadConfig_AutoUpdate_NoFlagWinsOverCloudDefault keeps the legacy CLI
// flag working: --no-auto-update (translated into overrides.DisableAutoUpdate)
// forces auto-update off even when the cloud default and env var would enable.
func TestLoadConfig_AutoUpdate_NoFlagWinsOverCloudDefault(t *testing.T) {
	stageFakeAgent(t)
	t.Setenv("ORVILO_DAEMON_AUTO_UPDATE", "true")
	cfg, err := LoadConfig(Overrides{
		ServerURL:         "https://api.aspectlylabs.com",
		WorkspacesRoot:    t.TempDir(),
		DisableAutoUpdate: true,
	})
	if err != nil {
		t.Fatalf("LoadConfig: %v", err)
	}
	if cfg.AutoUpdateEnabled {
		t.Fatalf("AutoUpdateEnabled = true with --no-auto-update set; flag must win")
	}
}

// TestLoadConfig_AutoReload_DefaultsOnEvenForSelfHost is the review's first
// product decision, encoded: "don't pull new versions from GitHub" and "follow
// the binary I replaced myself" are separate concerns. Self-host defaults
// auto-update OFF (MUL-2381) because upgrading a fork from an upstream release
// would clobber it — an argument that says nothing about a binary the operator
// installed by hand.
func TestLoadConfig_AutoReload_DefaultsOnEvenForSelfHost(t *testing.T) {
	stageFakeAgent(t)
	t.Setenv("ORVILO_DAEMON_AUTO_UPDATE", "")
	t.Setenv("ORVILO_DAEMON_AUTO_RELOAD", "")
	cfg, err := LoadConfig(Overrides{
		ServerURL:      "http://localhost:8080",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig: %v", err)
	}
	if cfg.AutoUpdateEnabled {
		t.Fatalf("AutoUpdateEnabled = true for self-host, want false (MUL-2381)")
	}
	if !cfg.AutoReloadEnabled {
		t.Fatalf("AutoReloadEnabled = false for self-host; the on-disk watcher must not ride on the auto-update default")
	}
}

// TestLoadConfig_AutoReload_NotGatedOnAutoUpdateEnv is the same decoupling at
// the env layer: turning GitHub polling off must not silently stop the daemon
// from following a hand-installed binary.
func TestLoadConfig_AutoReload_NotGatedOnAutoUpdateEnv(t *testing.T) {
	stageFakeAgent(t)
	t.Setenv("ORVILO_DAEMON_AUTO_UPDATE", "false")
	t.Setenv("ORVILO_DAEMON_AUTO_RELOAD", "")
	cfg, err := LoadConfig(Overrides{
		ServerURL:      "https://api.aspectlylabs.com",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig: %v", err)
	}
	if !cfg.AutoReloadEnabled {
		t.Fatalf("ORVILO_DAEMON_AUTO_UPDATE=false disabled auto-reload; the two switches are independent")
	}
}

// TestLoadConfig_AutoReload_OffSwitches pins the escape hatch across both layers
// it can be turned off from. The config-file layer resolves to
// overrides.DisableAutoReload in cmd_daemon.go, so it is covered by the same
// assertion as the flag.
func TestLoadConfig_AutoReload_OffSwitches(t *testing.T) {
	cases := []struct {
		name      string
		env       string
		overrides Overrides
	}{
		{name: "env false", env: "false"},
		{name: "env 0", env: "0"},
		{name: "env off", env: "off"},
		{name: "flag or config file", env: "", overrides: Overrides{DisableAutoReload: true}},
		{name: "flag beats a truthy env", env: "true", overrides: Overrides{DisableAutoReload: true}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			stageFakeAgent(t)
			t.Setenv("ORVILO_DAEMON_AUTO_RELOAD", tc.env)
			overrides := tc.overrides
			overrides.ServerURL = "https://api.aspectlylabs.com"
			overrides.WorkspacesRoot = t.TempDir()
			cfg, err := LoadConfig(overrides)
			if err != nil {
				t.Fatalf("LoadConfig: %v", err)
			}
			if cfg.AutoReloadEnabled {
				t.Fatalf("AutoReloadEnabled = true, want false")
			}
		})
	}
}

func TestLoadConfig_UsesCodexDesktopAppBundleFallback(t *testing.T) {
	pathDir := t.TempDir()
	fakeCodex := filepath.Join(pathDir, "Codex.app", "Contents", "Resources", "codex")
	if err := os.MkdirAll(filepath.Dir(fakeCodex), 0o755); err != nil {
		t.Fatalf("mkdir fake Codex bundle: %v", err)
	}
	if err := os.WriteFile(fakeCodex, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatalf("write fake Codex bundle CLI: %v", err)
	}

	oldBundlePaths := codexDesktopAppBundlePaths
	codexDesktopAppBundlePaths = func() []string { return []string{fakeCodex} }
	t.Cleanup(func() { codexDesktopAppBundlePaths = oldBundlePaths })

	t.Setenv("PATH", t.TempDir())
	stubAgentInstallDirectories(t, nil)
	t.Setenv("ORVILO_DAEMON_ID", "11111111-1111-1111-1111-111111111111")
	t.Setenv("ORVILO_CODEX_MODEL", "gpt-5")
	pinNonCodexAgentsToMissingPaths(t)

	cfg, err := LoadConfig(Overrides{
		ServerURL:      "http://localhost:0",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig: %v", err)
	}
	got, ok := cfg.Agents["codex"]
	if !ok {
		t.Fatalf("expected codex agent from Desktop app bundle fallback, got %#v", cfg.Agents)
	}
	if got.Path != fakeCodex {
		t.Fatalf("codex path = %q, want %q", got.Path, fakeCodex)
	}
	if got.Model != "gpt-5" {
		t.Fatalf("codex model = %q, want gpt-5", got.Model)
	}
}

// Regression for #5205: after OpenAI moved the Desktop app to ChatGPT.app,
// Orvilo must resolve the bundled CLI under ChatGPT.app (and prefer it over
// the legacy Codex.app path when both exist).
func TestLoadConfig_UsesChatGPTAppBundleCodexPath(t *testing.T) {
	pathDir := t.TempDir()
	fakeChatGPT := filepath.Join(pathDir, "ChatGPT.app", "Contents", "Resources", "codex")
	fakeLegacy := filepath.Join(pathDir, "Codex.app", "Contents", "Resources", "codex")
	for _, p := range []string{fakeChatGPT, fakeLegacy} {
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatalf("mkdir: %v", err)
		}
		if err := os.WriteFile(p, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
			t.Fatalf("write fake CLI: %v", err)
		}
	}

	oldBundlePaths := codexDesktopAppBundlePaths
	// Prefer ChatGPT first, matching production ordering.
	codexDesktopAppBundlePaths = func() []string { return []string{fakeChatGPT, fakeLegacy} }
	t.Cleanup(func() { codexDesktopAppBundlePaths = oldBundlePaths })

	t.Setenv("PATH", t.TempDir())
	stubAgentInstallDirectories(t, nil)
	t.Setenv("ORVILO_DAEMON_ID", "11111111-1111-1111-1111-111111111111")
	pinNonCodexAgentsToMissingPaths(t)

	cfg, err := LoadConfig(Overrides{
		ServerURL:      "http://localhost:0",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig: %v", err)
	}
	got, ok := cfg.Agents["codex"]
	if !ok {
		t.Fatalf("expected codex agent from ChatGPT.app bundle, got %#v", cfg.Agents)
	}
	if got.Path != fakeChatGPT {
		t.Fatalf("codex path = %q, want ChatGPT.app path %q", got.Path, fakeChatGPT)
	}
}

func TestCodexDesktopAppBundlePaths_IncludesChatGPTAndLegacy(t *testing.T) {
	paths := codexDesktopAppBundlePaths()
	var hasChatGPT, hasLegacy bool
	for _, p := range paths {
		if strings.Contains(p, "ChatGPT.app") && strings.HasSuffix(filepath.ToSlash(p), "Contents/Resources/codex") {
			hasChatGPT = true
		}
		if strings.Contains(p, "Codex.app") && strings.HasSuffix(filepath.ToSlash(p), "Contents/Resources/codex") {
			hasLegacy = true
		}
	}
	if !hasChatGPT {
		t.Fatalf("codexDesktopAppBundlePaths missing ChatGPT.app entry: %#v", paths)
	}
	if !hasLegacy {
		t.Fatalf("codexDesktopAppBundlePaths missing legacy Codex.app entry: %#v", paths)
	}
	// New path must be preferred (listed before legacy).
	chatgptIdx, legacyIdx := -1, -1
	for i, p := range paths {
		if chatgptIdx < 0 && strings.Contains(p, "ChatGPT.app") {
			chatgptIdx = i
		}
		if legacyIdx < 0 && strings.Contains(p, "Codex.app") {
			legacyIdx = i
		}
	}
	if chatgptIdx < 0 || legacyIdx < 0 || chatgptIdx > legacyIdx {
		t.Fatalf("expected ChatGPT.app before Codex.app, got indices chat=%d legacy=%d paths=%#v", chatgptIdx, legacyIdx, paths)
	}
}

func TestLoadConfig_CodexDesktopFallbackDoesNotOverrideExplicitPath(t *testing.T) {
	pathDir := t.TempDir()
	fakeCodex := filepath.Join(pathDir, "Codex.app", "Contents", "Resources", "codex")
	if err := os.MkdirAll(filepath.Dir(fakeCodex), 0o755); err != nil {
		t.Fatalf("mkdir fake Codex bundle: %v", err)
	}
	if err := os.WriteFile(fakeCodex, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatalf("write fake Codex bundle CLI: %v", err)
	}

	oldBundlePaths := codexDesktopAppBundlePaths
	codexDesktopAppBundlePaths = func() []string { return []string{fakeCodex} }
	t.Cleanup(func() { codexDesktopAppBundlePaths = oldBundlePaths })

	t.Setenv("PATH", t.TempDir())
	stubAgentInstallDirectories(t, nil)
	t.Setenv("ORVILO_DAEMON_ID", "11111111-1111-1111-1111-111111111111")
	t.Setenv("ORVILO_CODEX_PATH", filepath.Join(t.TempDir(), "missing-codex"))
	pinNonCodexAgentsToMissingPaths(t)
	fakeClaude := filepath.Join(t.TempDir(), "claude")
	if err := os.WriteFile(fakeClaude, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatalf("write fake claude: %v", err)
	}
	t.Setenv("ORVILO_CLAUDE_PATH", fakeClaude)

	cfg, err := LoadConfig(Overrides{
		ServerURL:      "http://localhost:0",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig: %v", err)
	}
	if got, ok := cfg.Agents["codex"]; ok {
		t.Fatalf("explicit missing ORVILO_CODEX_PATH should not fall back to Desktop bundle, got %#v", got)
	}
}

func pinNonCodexAgentsToMissingPaths(t *testing.T) {
	t.Helper()
	missingDir := t.TempDir()
	for _, name := range []string{
		"ORVILO_CLAUDE_PATH",
		"ORVILO_OPENCODE_PATH",
		"ORVILO_OPENCLAW_PATH",
		"ORVILO_HERMES_PATH",
		"ORVILO_PI_PATH",
		"ORVILO_CURSOR_PATH",
		"ORVILO_COPILOT_PATH",
		"ORVILO_KIMI_PATH",
		"ORVILO_REASONIX_PATH",
		"ORVILO_DSH_PATH",
		"ORVILO_KIRO_PATH",
		"ORVILO_GROK_PATH",
	} {
		t.Setenv(name, filepath.Join(missingDir, strings.ToLower(name)))
	}
}

// =============================================================================
// CLI config Backends.OpenClaw overrides (issue #3875)
// =============================================================================

// writeCLIConfigForProfile is a minimal helper for the override tests:
// stages a HOME, writes a config.json under the given profile (empty profile
// = default), and returns the resolved path so tests can assert against it.
func writeCLIConfigForProfile(t *testing.T, profile string, cfg cli.CLIConfig) {
	t.Helper()
	tmp := t.TempDir()
	t.Setenv("HOME", tmp)
	if err := cli.SaveCLIConfigForProfile(cfg, profile); err != nil {
		t.Fatalf("write cli config: %v", err)
	}
}

// TestApplyOpenclawOverride_DoesNothingWhenNil verifies the early-return
// path. A daemon started with no override should not Setenv anything; the
// existing probe / spawn flow remains undisturbed.
func TestApplyOpenclawOverride_DoesNothingWhenNil(t *testing.T) {
	// Pre-set both env vars to known values; verify they survive untouched.
	t.Setenv("ORVILO_OPENCLAW_PATH", "/before/openclaw")
	t.Setenv("OPENCLAW_STATE_DIR", "/before/state")

	applyOpenclawOverride(nil)

	if got := os.Getenv("ORVILO_OPENCLAW_PATH"); got != "/before/openclaw" {
		t.Errorf("ORVILO_OPENCLAW_PATH mutated: got %q, want /before/openclaw", got)
	}
	if got := os.Getenv("OPENCLAW_STATE_DIR"); got != "/before/state" {
		t.Errorf("OPENCLAW_STATE_DIR mutated: got %q, want /before/state", got)
	}
}

// TestApplyOpenclawOverride_SetsBothWhenEnvUnset verifies the happy path:
// neither env var is set, the override has both fields, both env vars get
// set to the override values.
func TestApplyOpenclawOverride_SetsBothWhenEnvUnset(t *testing.T) {
	t.Setenv("ORVILO_OPENCLAW_PATH", "")
	t.Setenv("OPENCLAW_STATE_DIR", "")
	os.Unsetenv("ORVILO_OPENCLAW_PATH")
	os.Unsetenv("OPENCLAW_STATE_DIR")
	t.Cleanup(func() {
		os.Unsetenv("ORVILO_OPENCLAW_PATH")
		os.Unsetenv("OPENCLAW_STATE_DIR")
	})

	applyOpenclawOverride(&cli.OpenClawOverride{
		BinaryPath: "/from/config/openclaw",
		StateDir:   "/from/config/state",
	})

	if got := os.Getenv("ORVILO_OPENCLAW_PATH"); got != "/from/config/openclaw" {
		t.Errorf("ORVILO_OPENCLAW_PATH: got %q, want /from/config/openclaw", got)
	}
	if got := os.Getenv("OPENCLAW_STATE_DIR"); got != "/from/config/state" {
		t.Errorf("OPENCLAW_STATE_DIR: got %q, want /from/config/state", got)
	}
}

// TestApplyOpenclawOverride_EnvWinsOverConfig is the precedence test
// agreed with @YOMXXX in #3875 review: an env var set upstream by the user
// (shell export, launchctl, systemd unit) MUST take precedence over the
// config-file value. This is the back-compat contract — anyone with
// ORVILO_OPENCLAW_PATH already in their environment must not see the
// daemon silently change its meaning when they later add a config file.
func TestApplyOpenclawOverride_EnvWinsOverConfig(t *testing.T) {
	// User has already exported these in their shell.
	t.Setenv("ORVILO_OPENCLAW_PATH", "/from/env/openclaw")
	t.Setenv("OPENCLAW_STATE_DIR", "/from/env/state")

	applyOpenclawOverride(&cli.OpenClawOverride{
		BinaryPath: "/from/config/openclaw",
		StateDir:   "/from/config/state",
	})

	if got := os.Getenv("ORVILO_OPENCLAW_PATH"); got != "/from/env/openclaw" {
		t.Errorf("ORVILO_OPENCLAW_PATH: env should win, got %q want /from/env/openclaw", got)
	}
	if got := os.Getenv("OPENCLAW_STATE_DIR"); got != "/from/env/state" {
		t.Errorf("OPENCLAW_STATE_DIR: env should win, got %q want /from/env/state", got)
	}
}

// TestApplyOpenclawOverride_PartialFields_OnlySetsConfigured verifies that
// an override with only one field set leaves the other env var alone (does
// not Setenv to ""). This matters: a user who only configures state_dir
// must not have their ORVILO_OPENCLAW_PATH discovery path forcibly
// short-circuited to an empty string.
func TestApplyOpenclawOverride_PartialFields_OnlySetsConfigured(t *testing.T) {
	os.Unsetenv("ORVILO_OPENCLAW_PATH")
	os.Unsetenv("OPENCLAW_STATE_DIR")
	t.Cleanup(func() {
		os.Unsetenv("ORVILO_OPENCLAW_PATH")
		os.Unsetenv("OPENCLAW_STATE_DIR")
	})

	applyOpenclawOverride(&cli.OpenClawOverride{
		StateDir: "/from/config/state",
		// BinaryPath intentionally empty — must NOT call Setenv("ORVILO_OPENCLAW_PATH", "")
	})

	if _, set := os.LookupEnv("ORVILO_OPENCLAW_PATH"); set {
		t.Errorf("ORVILO_OPENCLAW_PATH should remain unset when BinaryPath is empty; got %q", os.Getenv("ORVILO_OPENCLAW_PATH"))
	}
	if got := os.Getenv("OPENCLAW_STATE_DIR"); got != "/from/config/state" {
		t.Errorf("OPENCLAW_STATE_DIR: got %q, want /from/config/state", got)
	}
}

// TestOpenclawOverrideFrom_NavigationCases verifies the nullable-pointer
// chain into Backends.OpenClaw. Three cases that all must safely return
// nil without panicking: nil Backends, nil OpenClaw inside Backends, and
// the happy path (returns the inner override unchanged).
func TestOpenclawOverrideFrom_NavigationCases(t *testing.T) {
	if got := openclawOverrideFrom(cli.CLIConfig{}); got != nil {
		t.Errorf("nil Backends should produce nil override, got %+v", got)
	}
	if got := openclawOverrideFrom(cli.CLIConfig{Backends: &cli.BackendOverrides{}}); got != nil {
		t.Errorf("nil OpenClaw inside Backends should produce nil override, got %+v", got)
	}
	want := &cli.OpenClawOverride{StateDir: "/x"}
	got := openclawOverrideFrom(cli.CLIConfig{Backends: &cli.BackendOverrides{OpenClaw: want}})
	if got != want {
		t.Errorf("happy path should return inner pointer; got %p want %p", got, want)
	}
}

// TestLoadConfig_AppliesBackendOverridesFromConfigFile is the integration
// test that ties commit 1's schema to commit 2's wire-up: write a config
// file with backends.openclaw.{binary_path,state_dir}, call LoadConfig
// (with no env vars set), and verify the openclaw probe picked up the
// configured BinaryPath and the OPENCLAW_STATE_DIR env var was injected.
func TestLoadConfig_AppliesBackendOverridesFromConfigFile(t *testing.T) {
	stageFakeAgent(t)
	// stageFakeAgent left "claude" on PATH; we also need a fake "openclaw"
	// at a custom path that the config file points at (mimicking a non-default
	// installation: another bundled / isolated / CI deployment, etc).
	customDir := t.TempDir()
	customOpenclaw := filepath.Join(customDir, "non-default-openclaw")
	if err := os.WriteFile(customOpenclaw, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatalf("write fake openclaw: %v", err)
	}

	// Make sure no env-var override is leaking in from the test runner.
	os.Unsetenv("ORVILO_OPENCLAW_PATH")
	os.Unsetenv("OPENCLAW_STATE_DIR")
	t.Cleanup(func() {
		os.Unsetenv("ORVILO_OPENCLAW_PATH")
		os.Unsetenv("OPENCLAW_STATE_DIR")
	})

	// Drop a CLI config under the user's HOME (already pointed at TempDir
	// by stageFakeAgent's t.Setenv chain — but reassert here for clarity).
	homeForCLIConfig := t.TempDir()
	t.Setenv("HOME", homeForCLIConfig)
	cfg := cli.CLIConfig{
		ServerURL: "http://localhost:8080",
		Backends: &cli.BackendOverrides{
			OpenClaw: &cli.OpenClawOverride{
				BinaryPath: customOpenclaw,
				StateDir:   "/var/lib/openclaw-isolated",
			},
		},
	}
	if err := cli.SaveCLIConfig(cfg); err != nil {
		t.Fatalf("save cli config: %v", err)
	}

	loaded, err := LoadConfig(Overrides{
		ServerURL:      "http://localhost:8080",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig: %v", err)
	}

	openclaw, ok := loaded.Agents["openclaw"]
	if !ok {
		t.Fatalf("agents map missing 'openclaw' key; got keys=%v", agentKeys(loaded.Agents))
	}
	if openclaw.Path != customOpenclaw {
		t.Errorf("openclaw.Path: got %q, want %q (the binary configured in CLI config)", openclaw.Path, customOpenclaw)
	}
	if got := os.Getenv("OPENCLAW_STATE_DIR"); got != "/var/lib/openclaw-isolated" {
		t.Errorf("OPENCLAW_STATE_DIR: got %q, want injected from config", got)
	}
}

// TestLoadConfig_BackendOverrides_BackwardCompat_NoConfigFile verifies that
// the override mechanism is purely additive: a daemon started without any
// CLI config file (or with an empty one) behaves identically to before
// commit 1 — agents discovered from PATH, no env injection.
func TestLoadConfig_BackendOverrides_BackwardCompat_NoConfigFile(t *testing.T) {
	stageFakeAgent(t)

	// Point HOME at an empty dir — no config.json present.
	t.Setenv("HOME", t.TempDir())
	os.Unsetenv("ORVILO_OPENCLAW_PATH")
	os.Unsetenv("OPENCLAW_STATE_DIR")
	t.Cleanup(func() {
		os.Unsetenv("ORVILO_OPENCLAW_PATH")
		os.Unsetenv("OPENCLAW_STATE_DIR")
	})

	_, err := LoadConfig(Overrides{
		ServerURL:      "http://localhost:8080",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig with no config file should not fail: %v", err)
	}

	if _, set := os.LookupEnv("OPENCLAW_STATE_DIR"); set {
		t.Errorf("OPENCLAW_STATE_DIR should remain unset when no config file is present; got %q", os.Getenv("OPENCLAW_STATE_DIR"))
	}
}

// TestLoadConfig_BackendOverrides_MalformedConfigFileNonFatal verifies the
// fail-soft contract documented inline in LoadConfig: a corrupt config.json
// must not prevent daemon startup. This matters for diskcorruption /
// partial-write recovery — the daemon should log and proceed using
// env-var-only configuration.
func TestLoadConfig_BackendOverrides_MalformedConfigFileNonFatal(t *testing.T) {
	stageFakeAgent(t)
	homeDir := t.TempDir()
	t.Setenv("HOME", homeDir)

	// Write malformed JSON.
	cfgDir := filepath.Join(homeDir, ".orvilo")
	if err := os.MkdirAll(cfgDir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(cfgDir, "config.json"), []byte("{not valid json"), 0o600); err != nil {
		t.Fatal(err)
	}

	_, err := LoadConfig(Overrides{
		ServerURL:      "http://localhost:8080",
		WorkspacesRoot: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("LoadConfig should not fail on malformed config.json: %v", err)
	}
	// Should also have logged a slog Warn — we don't assert on the log
	// output here (avoids brittle string matching), but the build does
	// make sure log/slog stays imported.
}

// agentKeys is a tiny helper to make agent-map missing-key error messages
// readable. Returns sorted keys.
func agentKeys(m map[string]AgentEntry) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

// TestApplyOpenclawOverride_CLITimeout covers the #7112 knob on the same
// precedence contract as binary_path / state_dir: the config file supplies it
// when the environment does not, and an environment value the user exported
// upstream always wins.
func TestApplyOpenclawOverride_CLITimeout(t *testing.T) {
	t.Run("config file supplies the value", func(t *testing.T) {
		os.Unsetenv(execenv.OpenclawCLITimeoutEnv)
		t.Cleanup(func() { os.Unsetenv(execenv.OpenclawCLITimeoutEnv) })

		applyOpenclawOverride(&cli.OpenClawOverride{CLITimeout: "45s"})

		if got := os.Getenv(execenv.OpenclawCLITimeoutEnv); got != "45s" {
			t.Errorf("%s: got %q, want 45s", execenv.OpenclawCLITimeoutEnv, got)
		}
	})

	t.Run("env wins over config", func(t *testing.T) {
		t.Setenv(execenv.OpenclawCLITimeoutEnv, "20s")

		applyOpenclawOverride(&cli.OpenClawOverride{CLITimeout: "45s"})

		if got := os.Getenv(execenv.OpenclawCLITimeoutEnv); got != "20s" {
			t.Errorf("%s: env should win, got %q want 20s", execenv.OpenclawCLITimeoutEnv, got)
		}
	})

	t.Run("unset field leaves the env alone", func(t *testing.T) {
		os.Unsetenv(execenv.OpenclawCLITimeoutEnv)
		t.Cleanup(func() { os.Unsetenv(execenv.OpenclawCLITimeoutEnv) })

		applyOpenclawOverride(&cli.OpenClawOverride{StateDir: "/from/config/state"})

		if _, set := os.LookupEnv(execenv.OpenclawCLITimeoutEnv); set {
			t.Errorf("%s must not be set when the field is empty", execenv.OpenclawCLITimeoutEnv)
		}
	})
}
