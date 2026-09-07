package handler

import (
	"net/http"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/service"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func (h *Handler) validateAutomationToolsForSave(w http.ResponseWriter, r *http.Request, raw []byte, workspaceID pgtype.UUID) bool {
	if err := service.ValidateAutomationTools(raw); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return false
	}
	cfg := service.ParseAutomationTools(raw)
	if cfg.SlackSend == nil {
		return true
	}
	installation, err := h.Queries.GetChannelInstallationInWorkspace(r.Context(), db.GetChannelInstallationInWorkspaceParams{
		ID: parseUUID(cfg.SlackSend.InstallationID), WorkspaceID: workspaceID, ChannelType: "slack",
	})
	if err != nil || installation.Status != "installed" {
		writeError(w, http.StatusBadRequest, "select a connected Slack workspace before enabling Slack output")
		return false
	}
	return true
}
