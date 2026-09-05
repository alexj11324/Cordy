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
