package main

import (
	"encoding/json"
	"testing"

	"github.com/patchbay-ai/patchbay/server/internal/events"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

func TestRegisterListeners_DependencyGraphUpdatedBroadcastsWorkspaceFrame(t *testing.T) {
	bus := events.New()
	broadcaster := &fakeBroadcaster{}
	registerListeners(bus, broadcaster)

	bus.Publish(events.Event{
		Type:        protocol.EventDependencyGraphUpdated,
		WorkspaceID: "workspace-1",
		ActorType:   "member",
		ActorID:     "member-1",
		Payload: map[string]any{
			"plan_id":         "plan-1",
			"parent_issue_id": "issue-1",
			"status":          "active",
		},
	})

	if len(broadcaster.workspaceCalls) != 1 {
		t.Fatalf("workspace broadcasts = %d, want 1", len(broadcaster.workspaceCalls))
	}
	if broadcaster.workspaceCalls[0].workspaceID != "workspace-1" {
		t.Fatalf("workspace id = %q, want workspace-1", broadcaster.workspaceCalls[0].workspaceID)
	}
	var frame struct {
		Type      string         `json:"type"`
		ActorType string         `json:"actor_type"`
		Payload   map[string]any `json:"payload"`
	}
	if err := json.Unmarshal(broadcaster.workspaceCalls[0].msg, &frame); err != nil {
		t.Fatalf("decode dependency graph frame: %v", err)
	}
	if frame.Type != protocol.EventDependencyGraphUpdated || frame.ActorType != "member" {
		t.Fatalf("frame identity = %#v", frame)
	}
	if frame.Payload["plan_id"] != "plan-1" || frame.Payload["parent_issue_id"] != "issue-1" || frame.Payload["status"] != "active" {
		t.Fatalf("frame payload = %#v", frame.Payload)
	}
}
