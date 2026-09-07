package handler

import (
	"net/http"
	"strconv"
	"strings"

	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

// ListWorkspaceAutomationRuns returns one page and scope-wide time-window
// counts. It is registered behind the same workspace-member middleware as
// ListAutomations; archived automations retain their historical runs here.
func (h *Handler) ListWorkspaceAutomationRuns(w http.ResponseWriter, r *http.Request) {
	workspaceID := parseUUID(h.resolveWorkspaceID(r))
	userID := parseUUID(requestUserID(r))
	mine := r.URL.Query().Get("scope") == "mine"
	search := strings.TrimSpace(r.URL.Query().Get("search"))
	statuses := r.URL.Query()["status"]
	if statuses == nil {
		statuses = []string{}
	}
	offset := int32(0)
	if raw := r.URL.Query().Get("offset"); raw != "" {
		value, err := strconv.ParseInt(raw, 10, 32)
		if err != nil || value < 0 {
			writeError(w, http.StatusBadRequest, "invalid offset")
			return
		}
		offset = int32(value)
	}
	summary, err := h.Queries.WorkspaceAutomationRunSummary(r.Context(), db.WorkspaceAutomationRunSummaryParams{
		WorkspaceID: workspaceID, UserID: userID, Mine: mine, Search: search, Statuses: statuses,
	})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to load run summary")
		return
	}
	rows, err := h.Queries.ListWorkspaceAutomationRuns(r.Context(), db.ListWorkspaceAutomationRunsParams{
		WorkspaceID: workspaceID, UserID: userID, Mine: mine, Search: search, Statuses: statuses, PageLimit: 25, PageOffset: offset,
	})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to list workspace runs")
		return
	}
	type overviewRun struct {
		AutomationRunResponse
		AutomationTitle string `json:"automation_title"`
		ExecutorID      string `json:"executor_id"`
	}
	runs := make([]overviewRun, len(rows))
	for i, row := range rows {
		runs[i] = overviewRun{
			AutomationRunResponse: AutomationRunResponse{
				ID:            uuidToString(row.ID),
				AutomationID:  uuidToString(row.AutomationID),
				TriggerID:     uuidToPtr(row.TriggerID),
				Source:        row.Source,
				Status:        row.Status,
				IssueID:       uuidToPtr(row.IssueID),
				TaskID:        uuidToPtr(row.TaskID),
				TriggeredAt:   timestampToString(row.TriggeredAt),
				CompletedAt:   timestampToPtr(row.CompletedAt),
				FailureReason: textToPtr(row.FailureReason),
				ReasonCode:    textToPtr(row.ReasonCode),
				Result:        jsonObjectOrNil(row.Result),
				CreatedAt:     timestampToString(row.CreatedAt),
			},
			AutomationTitle: row.AutomationTitle,
			ExecutorID:      uuidToString(row.ExecutorID),
		}
	}
	writeJSON(w, http.StatusOK, map[string]any{"runs": runs, "summary": summary})
}
