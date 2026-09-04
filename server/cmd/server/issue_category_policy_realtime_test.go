package main

import (
	"encoding/json"
	"testing"

	"github.com/patchbay-ai/patchbay/server/internal/events"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

func TestRegisterListeners_IssueCategoryPolicyChangedBroadcastsWorkspaceFrame(t *testing.T) {
	bus := events.New()
	broadcaster := &fakeBroadcaster{}
	registerListeners(bus, broadcaster)

	bus.Publish(events.Event{
		Type:        protocol.EventIssueCategoryPolicyChanged,
		WorkspaceID: "workspace-1",
		ActorType:   "member",
		ActorID:     "member-1",
		Payload:     map[string]any{"category": "in_review"},
	})

	if len(broadcaster.workspaceCalls) != 1 {
		t.Fatalf("workspace broadcasts = %d, want 1", len(broadcaster.workspaceCalls))
	}
	if broadcaster.workspaceCalls[0].workspaceID != "workspace-1" {
		t.Fatalf("workspace id = %q, want workspace-1", broadcaster.workspaceCalls[0].workspaceID)
	}
	var frame struct {
		Type      string         `json:"type"`
		ActorID   string         `json:"actor_id"`
		ActorType string         `json:"actor_type"`
		Payload   map[string]any `json:"payload"`
	}
	if err := json.Unmarshal(broadcaster.workspaceCalls[0].msg, &frame); err != nil {
		t.Fatalf("decode policy frame: %v", err)
	}
	if frame.Type != protocol.EventIssueCategoryPolicyChanged || frame.ActorID != "member-1" || frame.ActorType != "member" {
		t.Fatalf("frame identity = %#v", frame)
	}
	if frame.Payload["category"] != "in_review" {
		t.Fatalf("frame payload = %#v, want in_review", frame.Payload)
	}
}
