package handler

import (
	"encoding/json"
	"errors"
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5/pgtype"
	obsmetrics "github.com/patchbay-ai/patchbay/server/internal/metrics"
	"github.com/patchbay-ai/patchbay/server/internal/service"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

type agentThreadAvailabilityResponse struct {
	State      string `json:"state"`
	ReasonCode string `json:"reason_code,omitempty"`
	Reason     string `json:"reason,omitempty"`
}

type agentThreadAgentResponse struct {
	ID        string  `json:"id"`
	Name      string  `json:"name"`
	AvatarURL *string `json:"avatar_url"`
}

type agentThreadResponse struct {
	Task          AgentTaskResponse               `json:"task"`
	ThreadTasks   []AgentTaskResponse             `json:"thread_tasks"`
	CurrentTaskID string                          `json:"current_task_id"`
	Agent         agentThreadAgentResponse        `json:"agent"`
	Events        []protocol.TaskMessagePayload   `json:"events"`
	Availability  agentThreadAvailabilityResponse `json:"availability"`
	CanContinue   bool                            `json:"can_continue"`
}

type continueAgentThreadRequest struct {
	Content string `json:"content"`
}

type agentThreadAccess struct {
	tasks     []db.AgentTaskQueue
	agent     db.Agent
	canInvoke bool
	requester pgtype.UUID
}

func agentThreadReason(reason service.AgentThreadUnavailableReason) string {
	switch reason {
	case service.AgentThreadProviderSessionRetired:
		return "The provider session was deleted or retired and cannot be restored."
	case service.AgentThreadProviderSessionMissing:
		return "The provider session data is missing, so this Agent thread cannot continue safely."
	case service.AgentThreadFreshSessionRequired:
		return "This run requires a fresh provider session and cannot continue the previous thread."
	case service.AgentThreadProviderSessionNotEstablished:
		return "The provider has not established a session for this run yet."
	case service.AgentThreadAgentArchived:
		return "This Agent is archived and its thread cannot continue."
	case service.AgentThreadAgentRuntimeUnbound:
		return "This Agent is no longer bound to a runtime, so its thread cannot continue."
	case service.AgentThreadAgentRuntimeRebound:
		return "This Agent is bound to a different runtime, so its thread cannot continue safely."
	case service.AgentThreadAgentRuntimeMissing:
		return "The Agent runtime no longer exists, so its thread cannot continue."
	default:
		return "This Agent thread cannot continue safely."
	}
}

func (h *Handler) loadAgentThreadAccess(w http.ResponseWriter, r *http.Request) (agentThreadAccess, bool) {
	userID, ok := requireUserID(w, r)
	if !ok {
		return agentThreadAccess{}, false
	}
	workspaceID := ctxWorkspaceID(r.Context())
	wsUUID, ok := parseUUIDOrBadRequest(w, workspaceID, "workspace id")
	if !ok {
		return agentThreadAccess{}, false
	}
	taskUUID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "taskId"), "task id")
	if !ok {
		return agentThreadAccess{}, false
	}
	task, err := h.Queries.GetAgentTaskInWorkspace(r.Context(), db.GetAgentTaskInWorkspaceParams{ID: taskUUID, WorkspaceID: wsUUID})
	if err != nil || !task.IssueID.Valid || task.ChatSessionID.Valid || task.AutomationRunID.Valid {
		writeError(w, http.StatusNotFound, "task conversation not found")
		return agentThreadAccess{}, false
	}
	agent, err := h.Queries.GetAgentInWorkspace(r.Context(), db.GetAgentInWorkspaceParams{ID: task.AgentID, WorkspaceID: wsUUID})
	if err != nil {
		writeError(w, http.StatusNotFound, "task conversation not found")
		return agentThreadAccess{}, false
	}
	actorType, actorID := h.resolveActor(r, userID, workspaceID)
	if !h.canAccessPrivateAgent(r.Context(), agent, actorType, actorID, workspaceID) {
		writeError(w, http.StatusForbidden, "you do not have access to this Agent thread")
		return agentThreadAccess{}, false
	}
	tasks, err := h.Queries.ListAgentThreadTasks(r.Context(), taskUUID)
	if err != nil || len(tasks) == 0 {
		writeError(w, http.StatusInternalServerError, "failed to load Agent thread")
		return agentThreadAccess{}, false
	}
	return agentThreadAccess{
		tasks:     tasks,
		agent:     agent,
		canInvoke: h.canInvokeAgent(r.Context(), agent, actorType, actorID, userID, workspaceID),
		requester: parseUUID(userID),
	}, true
}

func (h *Handler) GetAgentThread(w http.ResponseWriter, r *http.Request) {
	access, ok := h.loadAgentThreadAccess(w, r)
	if !ok {
		return
	}
	current := access.tasks[len(access.tasks)-1]
	availability := agentThreadAvailabilityResponse{State: "available"}
	canContinue := access.canInvoke
	if !access.canInvoke {
		availability.ReasonCode = "agent_thread_invoke_forbidden"
		availability.Reason = "You can read this Agent thread, but you do not have permission to continue it."
	} else {
		runtime, runtimeErr := h.runtimeLookup(obsmetrics.RuntimeLookupSourceTask).Get(r.Context(), current.RuntimeID)
		err := service.AgentThreadBindingAvailability(current, access.agent, runtimeErr == nil && runtime.WorkspaceID == access.agent.WorkspaceID)
		if err == nil {
			err = service.AgentThreadAvailability(current)
		}
		var unavailable *service.AgentThreadUnavailableError
		if errors.As(err, &unavailable) {
			availability = agentThreadAvailabilityResponse{State: "unavailable", ReasonCode: string(unavailable.Reason), Reason: agentThreadReason(unavailable.Reason)}
			canContinue = false
		}
	}

	ids := make([]pgtype.UUID, 0, len(access.tasks))
	for _, task := range access.tasks {
		ids = append(ids, task.ID)
	}
	messages, err := h.Queries.ListTaskMessagesForTasks(r.Context(), ids)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to load Agent thread events")
		return
	}
	events := make([]protocol.TaskMessagePayload, 0, len(messages))
	for _, message := range messages {
		events = append(events, taskMessageToPayload(message, uuidToString(message.TaskID), uuidToString(current.IssueID)))
	}
	workspaceID := ctxWorkspaceID(r.Context())
	threadTasks := make([]AgentTaskResponse, 0, len(access.tasks))
	for _, task := range access.tasks {
		threadTasks = append(threadTasks, taskToResponse(task, workspaceID))
	}
	avatarURL := textToPtr(access.agent.AvatarUrl)
	writeJSON(w, http.StatusOK, agentThreadResponse{
		Task: taskToResponse(current, workspaceID), ThreadTasks: threadTasks,
		CurrentTaskID: uuidToString(current.ID),
		Agent:         agentThreadAgentResponse{ID: uuidToString(access.agent.ID), Name: access.agent.Name, AvatarURL: avatarURL},
		Events:        events, Availability: availability, CanContinue: canContinue,
	})
}

func (h *Handler) ContinueAgentThread(w http.ResponseWriter, r *http.Request) {
	access, ok := h.loadAgentThreadAccess(w, r)
	if !ok {
		return
	}
	if !access.canInvoke {
		writeError(w, http.StatusForbidden, "you do not have permission to continue this Agent thread")
		return
	}
	var request continueAgentThreadRequest
	if err := json.NewDecoder(r.Body).Decode(&request); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	idempotencyKey := r.Header.Get("Idempotency-Key")
	parent := access.tasks[len(access.tasks)-1]
	receipt, err := h.TaskService.ContinueAgentThread(r.Context(), parent.ID, request.Content, idempotencyKey, access.requester)
	if err == nil {
		status := "queued"
		if receipt.Coalesced {
			status = "coalesced"
		}
		writeJSON(w, http.StatusOK, map[string]string{"continuation_task_id": uuidToString(receipt.Task.ID), "status": status})
		return
	}
	var unavailable *service.AgentThreadUnavailableError
	switch {
	case errors.As(err, &unavailable):
		writeJSON(w, http.StatusConflict, map[string]string{"error": "agent_thread_unavailable", "reason_code": string(unavailable.Reason), "reason": agentThreadReason(unavailable.Reason)})
	case errors.Is(err, service.ErrAgentThreadIdempotencyConflict):
		writeJSON(w, http.StatusConflict, map[string]string{"error": "agent_thread_idempotency_conflict", "reason": err.Error()})
	case errors.Is(err, service.ErrAgentThreadDepthLimit):
		writeJSON(w, http.StatusConflict, map[string]string{"error": "agent_thread_depth_limit", "reason_code": "agent_thread_depth_limit", "reason": err.Error()})
	case errors.Is(err, service.ErrAgentThreadInvokeForbidden):
		writeError(w, http.StatusForbidden, err.Error())
	default:
		writeError(w, http.StatusBadRequest, err.Error())
	}
}
