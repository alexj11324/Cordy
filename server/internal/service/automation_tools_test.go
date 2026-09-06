package service

import (
	"strings"
	"testing"
)

func TestAutomationToolsDispatchNotes(t *testing.T) {
	if got := automationToolsDispatchNotes(nil); got != "" {
		t.Fatalf("empty tools: %q", got)
	}
	got := automationToolsDispatchNotes([]byte(`{"memories":{"enabled":true},"slack_send":{"enabled":true,"channel":"#eng"}}`))
	if !strings.Contains(got, "Memories are enabled") {
		t.Fatalf("got %q", got)
	}
	if !strings.Contains(got, "#eng") {
		t.Fatalf("got %q", got)
	}
}

func TestAutomationMCPServerAllowlistDistinguishesAbsentAndEmpty(t *testing.T) {
	if ids, configured, err := AutomationMCPServerAllowlist([]byte(`{}`)); err != nil || configured || ids != nil {
		t.Fatalf("absent allowlist = %#v, %v, %v", ids, configured, err)
	}
	if ids, configured, err := AutomationMCPServerAllowlist([]byte(`{"mcp_server_ids":null}`)); err != nil || configured || ids != nil {
		t.Fatalf("null allowlist = %#v, %v, %v", ids, configured, err)
	}
	if ids, configured, err := AutomationMCPServerAllowlist([]byte(`{"mcp_server_ids":[]}`)); err != nil || !configured || len(ids) != 0 {
		t.Fatalf("empty allowlist = %#v, %v, %v", ids, configured, err)
	}
	ids, configured, err := AutomationMCPServerAllowlist([]byte(`{"mcp_server_ids":[" a ","a",""]}`))
	if err != nil || !configured || len(ids) != 1 || ids[0] != "a" {
		t.Fatalf("normalized allowlist = %#v, %v, %v", ids, configured, err)
	}
}
