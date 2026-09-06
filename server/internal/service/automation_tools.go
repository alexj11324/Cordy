package service

import (
	"encoding/json"
	"fmt"
	"strings"
)

type AutomationToolsConfig struct {
	Memories *struct {
		Enabled bool `json:"enabled"`
	} `json:"memories,omitempty"`
	SlackSend *struct {
		Enabled bool   `json:"enabled"`
		Channel string `json:"channel,omitempty"`
	} `json:"slack_send,omitempty"`
	MCPServerIDs []string `json:"mcp_server_ids,omitempty"`
}

func ParseAutomationTools(raw []byte) AutomationToolsConfig {
	var cfg AutomationToolsConfig
	if len(raw) == 0 {
		return cfg
	}
	_ = json.Unmarshal(raw, &cfg)
	return cfg
}

// AutomationMCPServerAllowlist returns the IDs selected for an automation and
// whether the field was explicitly configured. The second result matters:
// `mcp_server_ids: []` means "allow no workspace MCP servers", while an old
// automation with no field keeps the executor's normal workspace bindings.
func AutomationMCPServerAllowlist(raw []byte) ([]string, bool, error) {
	if len(raw) == 0 {
		return nil, false, nil
	}
	var doc map[string]json.RawMessage
	if err := json.Unmarshal(raw, &doc); err != nil {
		return nil, false, fmt.Errorf("automation tools must be a JSON object")
	}
	value, configured := doc["mcp_server_ids"]
	if !configured || string(value) == "null" {
		// Treat null like an omitted optional field. Only an explicit [] is
		// the user's deny-all selection; this keeps older rows and generic
		// JSON clients from unexpectedly losing their normal MCP bindings.
		return nil, false, nil
	}
	var ids []string
	if err := json.Unmarshal(value, &ids); err != nil {
		return nil, true, fmt.Errorf("mcp_server_ids must be an array")
	}
	seen := make(map[string]struct{}, len(ids))
	clean := make([]string, 0, len(ids))
	for _, id := range ids {
		id = strings.TrimSpace(id)
		if id == "" {
			continue
		}
		if _, ok := seen[id]; ok {
			continue
		}
		seen[id] = struct{}{}
		clean = append(clean, id)
	}
	return clean, true, nil
}

func automationToolsDispatchNotes(raw []byte) string {
	cfg := ParseAutomationTools(raw)
	var notes []string
	if cfg.Memories != nil && cfg.Memories.Enabled {
		notes = append(notes, "Memories are enabled for this automation — persist durable facts the team should keep.")
	}
	if cfg.SlackSend != nil && cfg.SlackSend.Enabled {
		channel := strings.TrimSpace(cfg.SlackSend.Channel)
		if channel != "" {
			notes = append(notes, "When the run finishes, send a Slack summary to "+channel+".")
		} else {
			notes = append(notes, "When the run finishes, send a Slack summary to the connected workspace.")
		}
	}
	if len(cfg.MCPServerIDs) > 0 {
		notes = append(notes, "Prefer the MCP servers allowlisted on this automation.")
	}
	if len(notes) == 0 {
		return ""
	}
	return strings.Join(notes, " ")
}
