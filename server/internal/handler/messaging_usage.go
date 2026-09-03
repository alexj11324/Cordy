package handler

import (
	"net/http"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"github.com/patchbay-ai/patchbay/server/internal/channelquota"
)

type MessagingQuotaUsageResponse struct {
	Mode        string  `json:"mode"`
	Used        *int64  `json:"used"`
	Reserved    *int64  `json:"reserved"`
	Limit       *int64  `json:"limit"`
	PeriodStart *string `json:"period_start"`
	PeriodEnd   *string `json:"period_end"`
	ResetAt     *string `json:"reset_at"`
}

func (h *Handler) GetMessagingQuotaUsage(w http.ResponseWriter, r *http.Request) {
	workspaceID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	if h.TaskService == nil || h.DB == nil {
		writeErrorCode(w, http.StatusServiceUnavailable, "quota_unavailable", "hosted messaging usage is temporarily unavailable")
		return
	}
	policy := channelquota.ResolveUsage(
		r.Context(), h.TaskService.Entitlements, h.TaskService.ManagedMessaging,
		uuid.UUID(workspaceID.Bytes), time.Now(),
	)
	response := MessagingQuotaUsageResponse{Mode: string(policy.Mode)}
	if policy.Mode == channelquota.UsageDisabled || policy.Mode == channelquota.UsageUnavailable {
		writeJSON(w, http.StatusOK, response)
		return
	}
	usage, err := channelquota.CountTurns(r.Context(), h.DB, uuid.UUID(workspaceID.Bytes), policy.Window)
	if err != nil {
		writeErrorCode(w, http.StatusServiceUnavailable, "quota_unavailable", "hosted messaging usage is temporarily unavailable")
		return
	}
	periodStart := policy.Window.PeriodStart.Format(time.RFC3339)
	periodEnd := policy.Window.PeriodEnd.Format(time.RFC3339)
	resetAt := policy.Window.ResetAt.Format(time.RFC3339)
	response.Used = &usage.Used
	response.Reserved = &usage.Reserved
	response.PeriodStart = &periodStart
	response.PeriodEnd = &periodEnd
	response.ResetAt = &resetAt
	if policy.Mode == channelquota.UsageManaged {
		response.Limit = &policy.Window.Limit
	}
	writeJSON(w, http.StatusOK, response)
}
