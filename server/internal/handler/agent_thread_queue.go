package handler

import (
	"errors"
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/orvilo-ai/orvilo/server/internal/service"
)

type prioritizeAgentThreadTaskResponse struct {
	TaskID       string `json:"task_id"`
	ActiveTaskID string `json:"active_task_id"`
}

func (h *Handler) PrioritizeAgentThreadTask(w http.ResponseWriter, r *http.Request) {
	access, ok := h.loadAgentThreadAccess(w, r)
	if !ok {
		return
	}
	if !access.canInvoke {
		writeError(w, http.StatusForbidden, "you do not have permission to steer this Agent thread")
		return
	}
	selectedTaskID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "queuedTaskId"), "queued task id")
	if !ok {
		return
	}

	receipt, err := h.TaskService.PrioritizeAgentThreadTask(
		r.Context(), access.tasks[0].ID, selectedTaskID, access.requester,
	)
	if err == nil {
		writeJSON(w, http.StatusOK, prioritizeAgentThreadTaskResponse{
			TaskID: uuidToString(receipt.Task.ID), ActiveTaskID: uuidToString(receipt.ActiveTask.ID),
		})
		return
	}
	var unavailable *service.AgentThreadUnavailableError
	switch {
	case errors.Is(err, service.ErrAgentThreadInvokeForbidden):
		writeError(w, http.StatusForbidden, "you do not have permission to steer this Agent thread")
	case errors.Is(err, service.ErrAgentThreadSteerNoActive),
		errors.Is(err, service.ErrAgentThreadSteerTaskNotQueued):
		writeError(w, http.StatusConflict, err.Error())
	case errors.As(err, &unavailable):
		writeJSON(w, http.StatusConflict, map[string]string{
			"error": "agent_thread_unavailable", "reason_code": string(unavailable.Reason),
			"reason": agentThreadReason(unavailable.Reason),
		})
	default:
		writeError(w, http.StatusInternalServerError, "failed to steer Agent thread")
	}
}
