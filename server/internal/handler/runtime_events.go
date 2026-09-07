package handler

import (
	"context"

	"github.com/orvilo-ai/orvilo/server/internal/events"
	"github.com/orvilo-ai/orvilo/server/internal/service"
	"github.com/orvilo-ai/orvilo/server/internal/storage"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

// RuntimeEventPublisher is shared by HTTP teardown and background retention.
// It owns only post-commit projection capabilities, without retaining Handler.
type RuntimeEventPublisher struct {
	Storage   storage.Storage
	PublicURL string
	SignedCDN bool
	Tasks     *service.TaskService
	Bus       *events.Bus
}

func NewRuntimeEventPublisher(store storage.Storage, publicURL string, signedCDN bool, tasks *service.TaskService, bus *events.Bus) *RuntimeEventPublisher {
	return &RuntimeEventPublisher{Storage: store, PublicURL: publicURL, SignedCDN: signedCDN, Tasks: tasks, Bus: bus}
}

func (p *RuntimeEventPublisher) agentToResponse(agent db.Agent) AgentResponse {
	avatar := textToPtr(agent.AvatarUrl)
	if avatar != nil {
		resolved := resolveAvatarURL(*avatar, p.Storage, p.PublicURL, p.SignedCDN)
		avatar = &resolved
	}
	return agentToResponseWithAvatar(agent, avatar)
}

func (p *RuntimeEventPublisher) publish(kind, workspaceID, actorType, actorID string, payload any) {
	p.Bus.Publish(events.Event{Type: kind, WorkspaceID: workspaceID, ActorType: actorType, ActorID: actorID, Payload: payload})
}

func (p *RuntimeEventPublisher) PublishRuntimeTeardown(ctx context.Context, res service.RuntimeTeardownResult, wsID, actorType, actorID, action string, publishRuntimeRefresh bool) {
	// The teardown transaction has already committed when this post-commit
	// publisher is called. Delete only the system-agent chat objects collected
	// inside that transaction; user-agent sessions and task history survive.
	if p.Storage != nil && len(res.AttachmentURLs) > 0 {
		keys := make([]string, len(res.AttachmentURLs))
		for i, raw := range res.AttachmentURLs {
			keys[i] = p.Storage.KeyFromURL(raw)
		}
		p.Storage.DeleteKeys(ctx, keys)
	}
	if p.Tasks != nil && len(res.CancelledTasks) > 0 {
		// The teardown deletes the runtime's system agents, and a system agent's
		// chat sessions go with it, so the workspace of a cancelled chat task is
		// no longer resolvable from the task row. It is this workspace.
		p.Tasks.BroadcastCancelledTasks(ctx, wsID, res.CancelledTasks)
	}
	for _, a := range res.UnboundAgents {
		// agent:status is the generic "this agent changed" broadcast the agent
		// update path already uses; subscribers refresh the row and see
		// runtime_bound=false. No agent:archived here — nothing was archived.
		p.publish(protocol.EventAgentStatus, wsID, actorType, actorID, map[string]any{
			"agent": broadcastAgentResponse(p.agentToResponse(a)),
		})
	}
	for _, a := range res.PausedAutomations {
		p.publish(protocol.EventAutomationUpdated, wsID, actorType, actorID, map[string]any{
			"automation": automationToResponse(a, nil),
		})
	}
	if publishRuntimeRefresh {
		p.PublishRuntimeRefresh(wsID, actorType, actorID, action)
	}
}

// PublishRuntimeRefresh asks connected clients to refetch runtime state.
func (p *RuntimeEventPublisher) PublishRuntimeRefresh(wsID, actorType, actorID, action string) {
	p.publish(protocol.EventDaemonRegister, wsID, actorType, actorID, map[string]any{"action": action})
}
