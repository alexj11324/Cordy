package service

import (
	"strings"
	"testing"
)

func TestAutomationToolsDispatchNotes(t *testing.T) {
	if got := AutomationToolsDispatchNotes("automation-id", nil); got != "" {
		t.Fatalf("empty tools: %q", got)
	}
	got := AutomationToolsDispatchNotes("automation-id", []byte(`{"memories":{"enabled":true},"slack_send":{"enabled":true,"installation_id":"11111111-1111-1111-1111-111111111111","channel_ids":["C123"]}}`))
	if !strings.Contains(got, "Memories are enabled") {
		t.Fatalf("got %q", got)
	}
	if !strings.Contains(got, "C123") || !strings.Contains(got, "automatically sends") || !strings.Contains(got, "do not send a duplicate") {
		t.Fatalf("got %q", got)
	}
	if got := AutomationToolsDispatchNotes("automation-id", []byte(`{"memories":{"enabled":false},"slack_send":{"enabled":false}}`)); got != "" {
		t.Fatalf("disabled legacy tools produced notes: %q", got)
	}
	for _, command := range []string{"memory list automation-id", "memory read automation-id", "memory write automation-id", "--revision", "outside the repository"} {
		if !strings.Contains(got, command) {
			t.Fatalf("missing %q in %q", command, got)
		}
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

func TestValidateAutomationToolsRequiresSlackDestination(t *testing.T) {
	for _, raw := range []string{
		`{"slack_send":{"enabled":true}}`,
		`{"slack_send":{"enabled":true,"channel":"#eng"}}`,
		`{"slack_send":{"enabled":true,"installation_id":"bad","channel_ids":["C123"]}}`,
		`{"slack_send":{"enabled":true,"installation_id":"11111111-1111-1111-1111-111111111111","channel_ids":["#eng"]}}`,
		`{"slack_send":{"enabled":true,"installation_id":"11111111-1111-1111-1111-111111111111","channel_ids":["C123","C123"]}}`,
	} {
		if err := ValidateAutomationTools([]byte(raw)); err == nil {
			t.Fatalf("expected invalid destination: %s", raw)
		}
	}
	for _, raw := range []string{
		`{}`, `{"memories":{"enabled":false}}`,
		`{"slack_send":{"enabled":false,"installation_id":"11111111-1111-1111-1111-111111111111","channel_ids":["C123"]}}`,
		`{"slack_send":{"enabled":true,"installation_id":"11111111-1111-1111-1111-111111111111","channel_ids":["C123","G234"]}}`,
	} {
		if err := ValidateAutomationTools([]byte(raw)); err != nil {
			t.Fatalf("valid config rejected: %v", err)
		}
	}
}
