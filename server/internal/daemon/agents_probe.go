package daemon

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/orvilo-ai/orvilo/server/pkg/agent"
)

// probeAgentCLIs discovers which built-in agent CLIs are installed on this
// machine and returns one AgentEntry per provider that resolved.
//
// This is pure discovery: no version detection and no minimum-version gate
// (detectBuiltinRuntimes owns those, per registration round). The result is
// therefore the machine's *availability* set, which is exactly what
// /health.agents reports and what `orvilo daemon probe-runtimes` prints.
//
// It is called once from LoadConfig at startup and again from the periodic
// workspace sync (refreshAgentAvailability), so a CLI the user installs while
// the daemon is already running gets picked up without a restart (MUL-5439).
// Everything it reads is process-external (PATH, ORVILO_*_PATH, ORVILO_*_MODEL),
// so re-running it is the only way to observe such an install.
//
// A var so tests can stub discovery without installing real CLIs.
var probeAgentCLIs = func() map[string]AgentEntry {
	// Discovery is a file lookup: inherited PATH, conventional install paths,
	// then provider-specific bundled executables. Never execute shell rc files.
	probe := func(envVar, defaultCmd, modelEnv string) (AgentEntry, bool) {
		cmd := envOrDefault(envVar, defaultCmd)
		if path, err := resolveAgentExecutablePath(cmd); err == nil {
			return AgentEntry{
				Path:    path,
				Command: cmd,
				Model:   strings.TrimSpace(os.Getenv(modelEnv)),
			}, true
		}
		// Install-path discovery only rescues bare command names. An operator
		// who pinned ORVILO_*_PATH to an absolute or relative path that
		// doesn't exist should hard-miss, not silently get a different
		// binary.
		if strings.ContainsAny(cmd, "/\\") {
			return AgentEntry{}, false
		}
		if path, ok := resolveAgentsFromInstallPaths([]string{cmd})[cmd]; ok {
			return AgentEntry{
				Path:    path,
				Command: cmd,
				Model:   strings.TrimSpace(os.Getenv(modelEnv)),
			}, true
		}
		if defaultCmd == "codex" && cmd == defaultCmd {
			// Codex Desktop bundles its CLI inside the macOS app instead of
			// installing it onto PATH.
			for _, p := range codexDesktopAppBundlePaths() {
				if _, err := os.Stat(p); err == nil {
					return AgentEntry{
						Path:    p,
						Command: cmd,
						Model:   strings.TrimSpace(os.Getenv(modelEnv)),
					}, true
				}
			}
		}
		return AgentEntry{}, false
	}

	agents := map[string]AgentEntry{}
	if e, ok := probe("ORVILO_CLAUDE_PATH", "claude", "ORVILO_CLAUDE_MODEL"); ok {
		agents["claude"] = e
	}
	if e, ok := probe("ORVILO_CODEX_PATH", "codex", "ORVILO_CODEX_MODEL"); ok {
		agents["codex"] = e
	}
	if e, ok := probe("ORVILO_OPENCODE_PATH", "opencode", "ORVILO_OPENCODE_MODEL"); ok {
		agents["opencode"] = e
	}
	if e, ok := probe("ORVILO_CODEARTS_PATH", "codearts", "ORVILO_CODEARTS_MODEL"); ok {
		agents["codearts"] = e
	} else if strings.TrimSpace(os.Getenv("ORVILO_CODEARTS_PATH")) == "" {
		// The native CodeArts installer may update PATH only for future
		// terminals. A GUI-launched daemon can still discover its stable
		// user-level launcher. An explicit but invalid override remains a hard
		// miss and never falls back here.
		home, err := os.UserHomeDir()
		if err == nil {
			for _, name := range []string{"codearts.cmd", "codearts"} {
				candidate := filepath.Join(home, ".codeartsdoer", "installers", name)
				path, resolveErr := resolveAgentExecutablePath(candidate)
				if resolveErr != nil {
					continue
				}
				agents["codearts"] = AgentEntry{
					Path:    path,
					Command: "codearts",
					Model:   strings.TrimSpace(os.Getenv("ORVILO_CODEARTS_MODEL")),
				}
				break
			}
		}
	}
	if e, ok := probe("ORVILO_DEVECO_PATH", "deveco", "ORVILO_DEVECO_MODEL"); ok {
		agents["deveco"] = e
	}
	if e, ok := probe("ORVILO_OPENCLAW_PATH", "openclaw", "ORVILO_OPENCLAW_MODEL"); ok {
		agents["openclaw"] = e
	}
	if e, ok := probe("ORVILO_HERMES_PATH", "hermes", "ORVILO_HERMES_MODEL"); ok {
		agents["hermes"] = e
	}
	if e, ok := probe("ORVILO_PI_PATH", "pi", "ORVILO_PI_MODEL"); ok {
		agents["pi"] = e
	}
	// Built-in runtime identities (e.g. omp) are derived from the descriptor
	// registry in server/pkg/agent/builtin_runtimes.go. Each one probes a
	// separate CLI independently so a host with both pi and omp installed gets
	// two runtimes. The env prefix and default command come from the
	// descriptor, so adding a new fork is a descriptor entry, not a probe edit.
	for _, desc := range agent.BuiltinRuntimes {
		pathEnv := desc.EnvPrefix + "_PATH"
		modelEnv := desc.EnvPrefix + "_MODEL"
		if e, ok := probe(pathEnv, desc.DefaultCommand, modelEnv); ok {
			agents[desc.ID] = e
		}
	}
	if e, ok := probe("ORVILO_CURSOR_PATH", "cursor-agent", "ORVILO_CURSOR_MODEL"); ok {
		agents["cursor"] = e
	}
	if e, ok := probe("ORVILO_COPILOT_PATH", "copilot", "ORVILO_COPILOT_MODEL"); ok {
		agents["copilot"] = e
	}
	if e, ok := probe("ORVILO_KIMI_PATH", "kimi", "ORVILO_KIMI_MODEL"); ok {
		agents["kimi"] = e
	}
	if e, ok := probe("ORVILO_REASONIX_PATH", "reasonix", "ORVILO_REASONIX_MODEL"); ok {
		agents["reasonix"] = e
	}
	// DSH is registered only when its Orvilo runtime profile is installed.
	// A bare dsh binary is not enough: without the bundle it has no --stdio
	// protocol and every task would fail after being advertised as healthy.
	if e, ok := probe("ORVILO_DSH_PATH", "dsh", "ORVILO_DSH_MODEL"); ok && probeDshOrviloProfile(e.Path) {
		agents["dsh"] = e
	}
	if e, ok := probe("ORVILO_KIRO_PATH", "kiro-cli", "ORVILO_KIRO_MODEL"); ok {
		agents["kiro"] = e
	}
	if e, ok := probe("ORVILO_CODEBUDDY_PATH", "codebuddy", "ORVILO_CODEBUDDY_MODEL"); ok {
		agents["codebuddy"] = e
	}
	// agy 1.0.6 added a `--model` flag (MUL-3125), so Antigravity now takes a
	// model env like every other backend. ORVILO_ANTIGRAVITY_MODEL seeds the
	// daemon-wide default; its value is the exact `agy models` display string
	// (e.g. "Claude Opus 4.6 (Thinking)"), not a provider/model slug.
	if e, ok := probe("ORVILO_ANTIGRAVITY_PATH", "agy", "ORVILO_ANTIGRAVITY_MODEL"); ok {
		agents["antigravity"] = e
	}
	// Qoder CLI ships as the `qodercli` binary (Qoder Desktop does not put it
	// on PATH; users install it separately, often via an npm global prefix).
	// Use the shared lookup so conventional Qoder installations are found
	// even when a GUI launch did not inherit their bin directory on PATH.
	if e, ok := probe("ORVILO_QODER_PATH", "qodercli", "ORVILO_QODER_MODEL"); ok {
		agents["qoder"] = e
	}
	// Qoder CN CLI exposes the same ACP transport as Qoder CLI under a
	// separate `qoderclicn` binary and account/config root. Register it as an
	// independent provider so hosts with either or both editions get the
	// matching runtime without a custom profile.
	if e, ok := probe("ORVILO_QODERCLICN_PATH", "qoderclicn", "ORVILO_QODERCLICN_MODEL"); ok {
		agents["qoderclicn"] = e
	}
	// ByteDance official TRAE CLI (the `traecli` binary from https://docs.trae.cn/cli),
	// driven over ACP via `traecli acp serve --yolo`. ORVILO_TRAECLI_MODEL seeds
	// the daemon-wide default model (a model id from the user's logged-in traecli
	// catalog).
	if e, ok := probe("ORVILO_TRAECLI_PATH", "traecli", "ORVILO_TRAECLI_MODEL"); ok {
		agents["traecli"] = e
	}
	// xAI Grok Build CLI (`grok`), driven over ACP via
	// `grok agent --always-approve stdio`. ORVILO_GROK_MODEL seeds the
	// daemon-wide default (e.g. grok-4.5).
	if e, ok := probe("ORVILO_GROK_PATH", "grok", "ORVILO_GROK_MODEL"); ok {
		agents["grok"] = e
	}
	// Qwen Code (`qwen`) runs headlessly with -p and stream-json. Its native
	// QWEN.md and .qwen/skills task context is prepared by execenv.
	if e, ok := probe("ORVILO_QWEN_PATH", "qwen", "ORVILO_QWEN_MODEL"); ok {
		agents["qwen"] = e
	}
	// QwenPaw (`qwenpaw`) is the QwenPaw CLI agent, driven over ACP via
	// `qwenpaw acp`. It takes no model env var: the backend never calls
	// session/set_model (it would rewrite QwenPaw's shared agent config), so
	// ExecOptions.Model is ignored — see ModelSelectionSupported. Reading one
	// here would only advertise a knob that silently does nothing.
	if e, ok := probe("ORVILO_QWENPAW_PATH", "qwenpaw", ""); ok {
		agents["qwenpaw"] = e
	}
	// Dim (`dim`) is the DimCode CLI agent, driven over ACP via `dim acp`.
	// ORVILO_DIM_MODEL seeds the daemon-wide default (a model id from the
	// user's logged-in dim catalog).
	if e, ok := probe("ORVILO_DIM_PATH", "dim", "ORVILO_DIM_MODEL"); ok {
		agents["dim"] = e
	}
	// MiniMax Code (`mcode`) exposes an ACP v1 server through `mcode acp`.
	// Model selection is owned by the MCode runtime, so there is no model env.
	if e, ok := probe("ORVILO_MCODE_PATH", "mcode", ""); ok {
		agents["mcode"] = e
	}
	// ZeroClaw (`zeroclaw`) is a Rust-based generic agent CLI, driven over
	// ACP via `zeroclaw acp`. It takes no model env var: its ACP server has no
	// `session/set_model` and no handler reads a model param, so the model
	// comes from ZeroClaw's own agent profile and ExecOptions.Model can never
	// be applied — see ModelSelectionSupported. Reading one here would only
	// advertise a knob that silently does nothing.
	if e, ok := probe("ORVILO_ZEROCLAW_PATH", "zeroclaw", ""); ok {
		agents["zeroclaw"] = e
	}
	return agents
}

// ProbeLocalAgents exposes the read-only discovery pass for local clients that
// must not load an Orvilo CLI profile. The regular daemon path still goes
// through LoadConfig so its profile-backed overrides remain intact.
func ProbeLocalAgents() map[string]AgentEntry {
	return probeAgentCLIs()
}

func probeDshOrviloProfile(executablePath string) bool {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, executablePath, "--profile", "orvilo", "--probe")
	cmd.WaitDelay = time.Second
	output, err := cmd.Output()
	if err != nil {
		return false
	}
	for _, line := range strings.Split(string(output), "\n") {
		var frame struct {
			Version         int    `json:"v"`
			Type            string `json:"type"`
			Runtime         string `json:"runtime"`
			ProtocolVersion int    `json:"protocol_version"`
		}
		if json.Unmarshal([]byte(line), &frame) == nil && frame.Version == 1 && frame.Type == "probe" && frame.Runtime == "dsh" && frame.ProtocolVersion == 1 {
			return true
		}
	}
	return false
}
