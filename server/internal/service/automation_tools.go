package service

import (
	"encoding/json"
	"strings"
)

type AutomationToolsConfig struct {
	Memories     *struct{ Enabled bool `json:"enabled"` } `json:"memories,omitempty"`
	SlackSend    *struct {
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
