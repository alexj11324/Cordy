package handler

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
)

// mcpServerContainers are the two top-level keys an mcp_config document may
// keep its servers under. `mcpServers` is the Claude-style container every
// runtime consumes; `mcp` is a legacy spelling a few agents still carry and
// which the agent settings UI keeps editing in place (mcp-config-model.ts).
// Both are read when deciding what an agent already declares; only
// `mcpServers` is ever written.
var mcpServerContainers = [...]string{"mcpServers", "mcp"}

// WorkspaceMcpBinding is one workspace MCP library entry to fold into a claim:
// the name it is mounted under plus its entry.
type WorkspaceMcpBinding struct {
	Name   string
	Config json.RawMessage
}

// ResolveAgentMcpConfig folds the given workspace MCP servers into an optional
// agent overlay and returns what the claim should carry. It runs before the
// per-task overlay (mergeMCPOverlay).
//
// The ordinary claim path passes every workspace library entry and a nil
// overlay, so leftover per-agent mcp_config cannot win name collisions or
// inject private servers. applyAutomationClaimSettings still passes an
// allowlisted subset plus a base overlay; empty automation allowlist stays
// deny-all in that caller.
//
// When an overlay is supplied, that overlay WINS on a name collision. An
// empty library and empty overlay resolve to nil — "nothing managed" — so
// the runtime keeps its native MCP inheritance.
//
// Shape: the result is NORMALIZED onto the canonical `mcpServers` container,
// including an overlay's legacy top-level `mcp` entries. Normalizing is
// mandatory, not cosmetic: the daemon's runtime merge only falls back to the
// legacy container when `mcpServers` is ABSENT
// (`internal/daemon/runtime_mcp.go`).
//
// Failure mode: on a malformed overlay the overlay is returned unchanged
// along with the error.
func ResolveAgentMcpConfig(bound []WorkspaceMcpBinding, agentMcpConfig json.RawMessage) (json.RawMessage, error) {
	if len(bound) == 0 {
		return passthroughAgentMcpConfig(agentMcpConfig), nil
	}

	shared := make(map[string]json.RawMessage, len(bound))
	for _, server := range bound {
		if server.Name == "" || !hasManagedJSON(server.Config) {
			continue
		}
		shared[server.Name] = server.Config
	}
	if len(shared) == 0 {
		return passthroughAgentMcpConfig(agentMcpConfig), nil
	}

	if !hasManagedJSON(agentMcpConfig) {
		// The agent declares nothing of its own: it runs with exactly the
		// servers it was given.
		out, err := json.Marshal(map[string]any{"mcpServers": shared})
		if err != nil {
			return nil, fmt.Errorf("resolve agent mcp_config: marshal bound servers: %w", err)
		}
		return out, nil
	}

	agentDoc, agentServers, err := parseMcpDocument(agentMcpConfig)
	if err != nil {
		return passthroughAgentMcpConfig(agentMcpConfig), fmt.Errorf("resolve agent mcp_config: parse agent mcp_config: %w", err)
	}

	merged := make(map[string]json.RawMessage, len(shared)+len(agentServers))
	for name, server := range shared {
		// The agent may declare this name in either container; agentServers
		// spans both, so this check covers each spelling.
		if _, taken := agentServers[name]; taken {
			continue
		}
		merged[name] = server
	}
	// Fold the agent's own entries in, legacy container first so a name present
	// in both resolves to the canonical one — the precedence the agent settings
	// UI already shows (mcp-config-model.ts reads `mcpServers` before `mcp`).
	for _, container := range [...]string{"mcp", "mcpServers"} {
		own, err := unmarshalServerMap(agentDoc[container])
		if err != nil {
			return passthroughAgentMcpConfig(agentMcpConfig), fmt.Errorf("resolve agent mcp_config: agent %s: %w", container, err)
		}
		for name, server := range own {
			merged[name] = server
		}
	}

	out := make(map[string]json.RawMessage, len(agentDoc)+1)
	for k, v := range agentDoc {
		// Both containers are consumed into the canonical map above; leaving
		// the legacy key behind would hand the daemon a second, stale copy.
		if k == "mcpServers" || k == "mcp" {
			continue
		}
		out[k] = v
	}
	serversBytes, err := json.Marshal(merged)
	if err != nil {
		return nil, fmt.Errorf("resolve agent mcp_config: marshal merged servers: %w", err)
	}
	out["mcpServers"] = serversBytes

	final, err := json.Marshal(out)
	if err != nil {
		return nil, fmt.Errorf("resolve agent mcp_config: marshal merged document: %w", err)
	}
	return final, nil
}

// parseMcpDocument decodes an mcp_config document into its top-level keys plus
// the set of server names it declares across BOTH containers. The name set is
// what "the agent already owns this name" keys off, so it must span the legacy
// spelling too.
func parseMcpDocument(raw json.RawMessage) (map[string]json.RawMessage, map[string]struct{}, error) {
	var doc map[string]json.RawMessage
	if err := json.Unmarshal(raw, &doc); err != nil {
		return nil, nil, err
	}
	if doc == nil {
		doc = map[string]json.RawMessage{}
	}
	names := map[string]struct{}{}
	for _, container := range mcpServerContainers {
		servers, err := unmarshalServerMap(doc[container])
		if err != nil {
			return nil, nil, fmt.Errorf("%s: %w", container, err)
		}
		for name := range servers {
			names[name] = struct{}{}
		}
	}
	return doc, names, nil
}

// validateWorkspaceMcpServerEntry checks the shape of ONE server entry before
// it is stored. Deliberately shallow — shape only, never the contents, which
// are runtime-specific and carry secrets we do not want to inspect or echo
// back in an error.
func validateWorkspaceMcpServerEntry(raw json.RawMessage) error {
	trimmed := bytes.TrimSpace(raw)
	if len(trimmed) == 0 {
		return errors.New("config must be a JSON object")
	}
	var entry map[string]json.RawMessage
	if err := json.Unmarshal(trimmed, &entry); err != nil {
		// Never wrap: the underlying error can echo fragments of an entry that
		// routinely embeds API tokens.
		return errors.New("config must be a JSON object")
	}
	if len(entry) == 0 {
		return errors.New("config must not be empty")
	}
	return nil
}

// validateWorkspaceMcpServerName checks a server name. The name is what the
// runtime mounts the server under and what an agent's own config collides
// with, so it follows the same rule the agent settings dialog enforces.
func validateWorkspaceMcpServerName(name string) error {
	if name == "" {
		return errors.New("name is required")
	}
	for _, r := range name {
		switch {
		case r >= 'a' && r <= 'z', r >= 'A' && r <= 'Z', r >= '0' && r <= '9', r == '-', r == '_':
		default:
			return errors.New("name may only contain letters, digits, hyphens, and underscores")
		}
	}
	return nil
}
