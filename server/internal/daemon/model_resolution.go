package daemon

import (
	"context"
	"log/slog"
	"strings"

	"github.com/orvilo-ai/orvilo/server/pkg/agent"
)

// runtimeDisplayNameOverrides maps a provider key to the human-facing runtime
// name when simple title-casing would read awkwardly. Providers not listed
// here fall back to capitalizing the key (claude → "Claude", codex → "Codex").
// Built-in runtime identities (from agent.BuiltinRuntimes) are seeded into
// this map at init so their display names stay in lockstep with the
// descriptor.
var runtimeDisplayNameOverrides = map[string]string{
	"codearts":   "CodeArts",
	"dsh":        "DeepSeek Harness",
	"traecli":    "Trae",
	"grok":       "Grok",
	"qoderclicn": "Qoder CN",
	"qwen":       "Qwen Code",
	"qwenpaw":    "QwenPaw",
	"mcode":      "MiniMax Code",
	"zeroclaw":   "ZeroClaw",
}

func init() {
	// Seed built-in runtime identity display names from the descriptor so
	// adding a new fork doesn't require editing this map by hand.
	for _, desc := range agent.BuiltinRuntimes {
		runtimeDisplayNameOverrides[desc.ID] = desc.DisplayName
	}
}

// providerDisplayName returns the human-facing runtime name for a provider key.
func providerDisplayName(name string) string {
	if name == "" {
		return name
	}
	if friendly, ok := runtimeDisplayNameOverrides[name]; ok {
		return friendly
	}
	return strings.ToUpper(name[:1]) + name[1:]
}

// providerNeedsInlineSystemPrompt reports whether the runtime brief must ride
// along in the turn itself (agent.ExecOptions.SystemPrompt) because the CLI
// will not pick up the per-task context file execenv writes into the workdir.
// This is the ONLY place that decides it, and it is the reason every other
// backend sees an empty SystemPrompt.
//
// Adding a provider here is a real fix only when that CLI genuinely ignores its
// context file — traecli was added because it reads .trae/rules/ and not
// AGENTS.md, so its agents were silently missing the workflow section and left
// issues stuck in `todo` with no comment and no error. Adding one that DOES
// read the file just duplicates the brief on every turn.
//
// Confirmed to load their context file, so deliberately absent here. MUL-5392
// probed each one over its real launch path with a canary in the context file
// and no inline delivery: claude 2.1.220 (CLAUDE.md), codex 0.144.6 driving the
// app-server (AGENTS.md), opencode 1.17.7 (AGENTS.md), pi 0.67.2 (AGENTS.md),
// hermes 0.18.2 over ACP (AGENTS.md). MCode 0.1.2 also loads AGENTS.md by its
// native runtime contract. kiro was confirmed earlier by a kiro-cli
// 2.13.0 ACP smoke — see the call site. Still unprobed: grok, qoder, codebuddy.
func providerNeedsInlineSystemPrompt(provider string) bool {
	switch provider {
	case "openclaw", "kimi", "traecli", "qwenpaw":
		return true
	default:
		return false
	}
}

// taskModelSelection is what a task actually launches with: the model
// selector plus the capability overrides that survived validation.
type taskModelSelection struct {
	Model         string
	ThinkingLevel string
	ServiceTier   string
}

// resolveTaskModelSelection settles the model selector and its capability
// overrides against the runtime's own model catalog, reading that catalog at
// most once per task — and not at all when nothing needs it.
//
// The single read is the point. Discovery is a CLI subprocess with a 15-30s
// ceiling, and cachedDiscovery deliberately does not memoize a result that
// came back empty or as a fallback (#3729, MUL-5549) so a transient failure
// can retry immediately. A logged-out or timing-out runtime therefore pays
// that ceiling in full on every read, and a task that read the catalog once to
// qualify its model and again to validate thinking_level would pay it twice
// before the agent even starts (MUL-6471 review).
//
// Who asks for the catalog:
//   - opencode and its DevEco fork cannot execute an unqualified selector, so
//     a pinned model has to be resolved against the catalog before launch.
//   - thinking_level / service_tier are catalog-owned and keyed on the
//     catalog's own model id, so they need it whenever they are set.
//
// A pi task with no capability override asks for neither: pi's own resolver
// accepts the persisted id in every shape it can take, so the daemon has
// nothing to add and skips discovery entirely. Same for claude, codex, and any
// task that pins no model.
//
// Qualification runs first because both checks below match on the catalog's
// canonical id: an unqualified id silently fails every lookup and drops a
// perfectly valid level (GH #7300).
func resolveTaskModelSelection(
	ctx context.Context,
	provider string,
	runtimeCmd agent.Command,
	sel taskModelSelection,
	taskLog *slog.Logger,
) taskModelSelection {
	capabilityChecksPending := sel.ThinkingLevel != "" || sel.ServiceTier != ""

	read := false
	var (
		catalog    agent.Catalog
		catalogErr error
	)
	loadCatalog := func() (agent.Catalog, error) {
		if !read {
			read = true
			catalog, catalogErr = listModels(ctx, provider, runtimeCmd)
		}
		return catalog, catalogErr
	}

	sel.Model = qualifyTaskModel(provider, sel.Model, capabilityChecksPending, loadCatalog, taskLog)

	// service_tier is catalog-owned. Codex advertises tiers from
	// `codex debug models`; ACP runtimes advertise them from session/new.
	// As with thinking_level, stale or incompatible persisted values degrade
	// to the runtime default instead of failing the task. Catalog lookup
	// errors pass through so a transient discovery failure does not silently
	// disable a previously valid user choice.
	if sel.ServiceTier != "" {
		ok, err := agent.ValidateServiceTierWith(loadCatalog, provider, sel.Model, sel.ServiceTier)
		if err != nil {
			taskLog.Warn("service_tier: catalog lookup failed; passing through",
				"provider", provider,
				"model", sel.Model,
				"service_tier", sel.ServiceTier,
				"error", err,
			)
		} else if !ok {
			taskLog.Warn("service_tier: not valid for this (provider, model); skipping injection",
				"provider", provider,
				"model", sel.Model,
				"service_tier", sel.ServiceTier,
			)
			sel.ServiceTier = ""
		}
	}
	// Per-model guard: the server validates the literal token against the
	// provider's enum, but per-model gaps (Claude's `xhigh` on a non-Opus
	// model, Codex's per-model `supported_reasoning_levels`) only resolve
	// here, against the daemon's local CLI catalog. Invalid combinations
	// log a warning and drop the level rather than failing the task, so a
	// stale persisted value never blocks execution. An empty model is
	// resolved by ValidateThinkingLevelWith to the provider's default model so
	// default-model tasks aren't misjudged — except for codex, whose empty
	// model follows config.toml (any model) and so fails closed, dropping the
	// level here without a catalog read at all. Discovery errors fail open for
	// resolved models: if we can't list models, we keep the persisted level
	// and let the CLI object.
	if sel.ThinkingLevel != "" {
		ok, err := agent.ValidateThinkingLevelWith(loadCatalog, provider, sel.Model, sel.ThinkingLevel)
		if err != nil {
			taskLog.Warn("thinking_level: catalog lookup failed; passing through",
				"provider", provider,
				"model", sel.Model,
				"thinking_level", sel.ThinkingLevel,
				"error", err,
			)
		} else if !ok {
			taskLog.Warn("thinking_level: not valid for this (provider, model); skipping injection",
				"provider", provider,
				"model", sel.Model,
				"thinking_level", sel.ThinkingLevel,
			)
			sel.ThinkingLevel = ""
		}
	}

	return sel
}

// qualifyTaskModel promotes a persisted model id to the canonical
// `<provider>/<id>` selector its runtime catalog advertises, and returns the
// model to actually launch with.
//
// agent.model holds whatever was persisted, and for gateway-style providers a
// bare model id is itself slash-shaped (`claude/claude-opus-5` under provider
// `patchbay-anthropic`), so the delimiter cannot tell a missing provider from a
// present one — only the catalog knows (GH #7300).
//
// It reads the catalog for two distinct reasons, and neither is "because we
// can": either the runtime refuses to launch without a qualified selector, or
// a capability check is about to read the catalog anyway and will match
// nothing unless the id is canonical first. When neither holds, the persisted
// value goes to the CLI untouched and no subprocess is spawned.
func qualifyTaskModel(
	provider, model string,
	capabilityChecksPending bool,
	loadCatalog func() (agent.Catalog, error),
	taskLog *slog.Logger,
) string {
	if model == "" {
		return model
	}
	if !agent.ModelSelectorMustBeProviderQualified(provider) && !capabilityChecksPending {
		return model
	}
	catalog, err := loadCatalog()
	if err != nil {
		// Same fail-open posture as the capability checks: an unreachable
		// catalog must not stop a task whose model may well be exactly what
		// the CLI expects.
		taskLog.Warn("model: catalog lookup failed; using the configured model as-is",
			"provider", provider,
			"model", model,
			"error", err,
		)
		return model
	}
	qualified, rewritten := agent.QualifyModelID(catalog, model)
	if !rewritten {
		return model
	}
	taskLog.Info("model: qualified against the runtime catalog",
		"provider", provider,
		"configured_model", model,
		"model", qualified,
	)
	return qualified
}

func defaultArgsForProvider(cfg Config, provider string) []string {
	var args []string
	switch provider {
	case "claude":
		args = cfg.ClaudeArgs
	case "codex":
		args = cfg.CodexArgs
	case "codebuddy":
		args = cfg.CodebuddyArgs
	case "qwen":
		args = cfg.QwenArgs
	case "qwenpaw":
		args = cfg.QwenpawArgs
	default:
		return nil
	}
	return append([]string(nil), args...)
}
