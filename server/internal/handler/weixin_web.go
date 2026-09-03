package handler

import (
	"encoding/json"
	"errors"
	"net/http"
	"strings"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5/pgtype"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/weixin"
	"github.com/patchbay-ai/patchbay/server/internal/util/secretbox"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

const (
	weixinBodyLimit                = 16 * 1024
	weixinInstallationCreatedEvent = "weixin_installation:created"
	weixinInstallationRevokedEvent = "weixin_installation:revoked"
)

type WeixinInstallationResponse struct {
	Runtime MessagingConnectionStatus `json:"runtime"`
	ID              string `json:"id"`
	WorkspaceID     string `json:"workspace_id"`
	AgentID         string `json:"agent_id"`
	BotID           string `json:"bot_id"`
	ILinkUserID     string `json:"ilink_user_id"`
	InstallerUserID string `json:"installer_user_id"`
	Status          string `json:"status"`
	InstallationStatus string `json:"installation_status"`
	InstalledAt     string `json:"installed_at"`
	CreatedAt       string `json:"created_at"`
	UpdatedAt       string `json:"updated_at"`
}

func weixinInstallationToResponse(row db.ChannelInstallation) WeixinInstallationResponse {
	public := weixin.DecodePublicConfig(row.Config)
	legacyStatus, installationStatus := messagingInstallationWireStatuses(row.Status)
	return WeixinInstallationResponse{
		Runtime: initialConnectionStatus(row.Status),
		ID: uuidToString(row.ID), WorkspaceID: uuidToString(row.WorkspaceID), AgentID: uuidToString(row.AgentID),
		BotID: public.BotID, ILinkUserID: public.ILinkUserID, InstallerUserID: uuidToString(row.InstallerUserID),
		Status: legacyStatus, InstallationStatus: installationStatus,
		InstalledAt: row.InstalledAt.Time.UTC().Format(time.RFC3339),
		CreatedAt: row.CreatedAt.Time.UTC().Format(time.RFC3339), UpdatedAt: row.UpdatedAt.Time.UTC().Format(time.RFC3339),
	}
}

func (h *Handler) newWeixinInstallationService() (*weixin.InstallationService, error) {
	key, err := secretbox.LoadKey("PATCHBAY_WEIXIN_SECRET_KEY")
	if err != nil {
		return nil, err
	}
	box, err := secretbox.New(key)
	if err != nil {
		return nil, err
	}
	service, err := weixin.NewInstallationService(h.Queries, h.TxStarter, box, weixin.DefaultInstallSessionStore(), nil)
	if err != nil {
		return nil, err
	}
	// The QR finalize re-resolves the hosted cap through the limiter; nil on
	// self-hosted deployments keeps finalize uncapped.
	service.SetHostedCapacityLimiter(h.HostedCapacity)
	return service, nil
}

func (h *Handler) ListWeixinInstallations(w http.ResponseWriter, r *http.Request) {
	service, err := h.newWeixinInstallationService()
	if err != nil {
		if strings.Contains(err.Error(), "PATCHBAY_WEIXIN_SECRET_KEY is not set") {
			writeJSON(w, http.StatusOK, map[string]any{"installations": []WeixinInstallationResponse{}, "configured": false, "install_supported": false})
			return
		}
		writeError(w, http.StatusServiceUnavailable, "weixin integration not configured")
		return
	}
	workspaceID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	rows, err := service.ListByWorkspace(r.Context(), workspaceID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to list weixin installations")
		return
	}
	items := make([]WeixinInstallationResponse, 0, len(rows))
	for _, row := range rows {
		items = append(items, weixinInstallationToResponse(row))
	}
	ids := make([]string, 0, len(items))
	for _, item := range items {
		ids = append(ids, item.ID)
	}
	statuses, err := h.loadConnectionStatuses(r.Context(), workspaceID, ids)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to load connection status")
		return
	}
	for i := range items {
		items[i].Runtime = statuses[items[i].ID]
	}
	writeJSON(w, http.StatusOK, map[string]any{"installations": items, "configured": true, "install_supported": true})
}

func (h *Handler) BeginWeixinInstall(w http.ResponseWriter, r *http.Request) {
	service, err := h.newWeixinInstallationService()
	if err != nil {
		writeError(w, http.StatusServiceUnavailable, "weixin integration not configured")
		return
	}
	userID, ok := requireUserID(w, r)
	if !ok {
		return
	}
	workspaceID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	agentID, ok := parseUUIDOrBadRequest(w, strings.TrimSpace(r.URL.Query().Get("agent_id")), "agent id")
	if !ok {
		return
	}
	agent, err := h.Queries.GetAgentInWorkspace(r.Context(), db.GetAgentInWorkspaceParams{ID: agentID, WorkspaceID: workspaceID})
	if err != nil {
		writeError(w, http.StatusNotFound, "agent not found in this workspace")
		return
	}
	if !h.canManageAgent(w, r, agent) {
		return
	}
	initiatorID, ok := parseUUIDOrBadRequest(w, userID, "user id")
	if !ok {
		return
	}
	// Fail closed BEFORE the QR is issued: a workspace that cannot admit
	// another install should not start a scan flow at all. The finalize
	// re-resolves — this check only avoids a dead-end QR session.
	if _, ok := h.hostedInstallationLimit(w, r, workspaceID); !ok {
		return
	}
	result, err := service.Begin(r.Context(), weixin.BeginParams{WorkspaceID: workspaceID, AgentID: agentID, InitiatorID: initiatorID})
	if err != nil {
		writeError(w, http.StatusBadGateway, "failed to start Weixin authorization")
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"session_id": result.SessionID, "qr_code_url": result.QRCode,
		"expires_in_seconds":    weixin.InstallSessionTTLSeconds,
		"poll_interval_seconds": result.PollIntervalSeconds,
	})
}

func (h *Handler) GetWeixinInstallStatus(w http.ResponseWriter, r *http.Request) {
	service, err := h.newWeixinInstallationService()
	if err != nil {
		writeError(w, http.StatusServiceUnavailable, "weixin integration not configured")
		return
	}
	userID, ok := requireUserID(w, r)
	if !ok {
		return
	}
	workspaceID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	actorID, ok := parseUUIDOrBadRequest(w, userID, "user id")
	if !ok {
		return
	}
	result, err := service.Status(r.Context(), strings.TrimSpace(chi.URLParam(r, "sessionId")), workspaceID, actorID, r.URL.Query().Get("verify_code"))
	if err != nil {
		switch {
		case errors.Is(err, weixin.ErrInstallSessionForbidden):
			writeError(w, http.StatusForbidden, "install session is not yours")
		case errors.Is(err, weixin.ErrInstallSessionNotFound):
			writeError(w, http.StatusGone, "install session expired")
		case errors.Is(err, weixin.ErrUnsafeProviderURL), errors.Is(err, weixin.ErrConfirmationIncomplete):
			writeError(w, http.StatusBadGateway, "Weixin authorization response was invalid")
		case errors.Is(err, weixin.ErrBotOwnedByAnotherWorkspace), errors.Is(err, weixin.ErrBotOwnedBySameWorkspace), errors.Is(err, weixin.ErrBotOwnedByArchivedAgent), errors.Is(err, weixin.ErrBindingAlreadyAssigned):
			writeError(w, http.StatusConflict, "this Weixin account is already installed for another agent or workspace")
		case errors.Is(err, weixin.ErrInstallAuthorizationChanged):
			writeError(w, http.StatusForbidden, "authorization changed during install")
		default:
			if writeHostedCapacityError(w, err) {
				// The QR completed but the workspace is over its hosted
				// installation cap (or the cap could not be read) — the
				// session stays resumable, so a retry after capacity is
				// freed or Cloud recovers can still finalize it.
				return
			}
			writeError(w, http.StatusInternalServerError, "failed to save Weixin connection")
		}
		return
	}
	response := map[string]any{"status": result.Status}
	if result.InstallationID.Valid {
		response["installation_id"] = uuidToString(result.InstallationID)
	}
	if shouldPublishWeixinInstallationCreated(result) {
		h.publishWeixinInstallationCreated(result.InstallationID, userID, workspaceID)
	}
	writeJSON(w, http.StatusOK, response)
}

func shouldPublishWeixinInstallationCreated(result weixin.StatusResult) bool {
	return result.Created && result.Status == weixin.InstallStatusSuccess && result.InstallationID.Valid
}

func (h *Handler) publishWeixinInstallationCreated(installationID pgtype.UUID, actorID string, workspaceID pgtype.UUID) {
	h.publish(weixinInstallationCreatedEvent, uuidToString(workspaceID), "user", actorID, map[string]any{
		"id": uuidToString(installationID),
	})
}

func (h *Handler) RevokeWeixinInstallation(w http.ResponseWriter, r *http.Request) {
	service, err := h.newWeixinInstallationService()
	if err != nil {
		writeError(w, http.StatusServiceUnavailable, "weixin integration not configured")
		return
	}
	userID, ok := requireUserID(w, r)
	if !ok {
		return
	}
	workspaceID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	installationID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "installationId"), "installation id")
	if !ok {
		return
	}
	installation, err := service.GetInWorkspace(r.Context(), installationID, workspaceID)
	if err != nil {
		if errors.Is(err, weixin.ErrInstallationNotFound) {
			writeError(w, http.StatusNotFound, "weixin installation not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "failed to load installation")
		return
	}
	agent, agentErr := h.Queries.GetAgentInWorkspace(r.Context(), db.GetAgentInWorkspaceParams{ID: installation.AgentID, WorkspaceID: workspaceID})
	if agentErr != nil {
		if _, allowed := h.requireWorkspaceRole(w, r, uuidToString(workspaceID), "weixin installation not found", "owner", "admin"); !allowed {
			return
		}
	} else if !h.canManageAgent(w, r, agent) {
		return
	}
	if err := service.Revoke(r.Context(), installationID); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to revoke installation")
		return
	}
	h.publish(weixinInstallationRevokedEvent, uuidToString(workspaceID), "user", userID, map[string]any{"id": uuidToString(installationID)})
	w.WriteHeader(http.StatusNoContent)
}

type RedeemWeixinBindingTokenRequest struct {
	Token string `json:"token"`
}

type RedeemWeixinBindingTokenResponse struct {
	WorkspaceID    string `json:"workspace_id"`
	InstallationID string `json:"installation_id"`
	WeixinUserID   string `json:"weixin_user_id"`
}

func (h *Handler) RedeemWeixinBindingToken(w http.ResponseWriter, r *http.Request) {
	key, err := secretbox.LoadKey("PATCHBAY_WEIXIN_SECRET_KEY")
	if err != nil {
		writeError(w, http.StatusServiceUnavailable, "weixin integration not configured")
		return
	}
	if _, err := secretbox.New(key); err != nil {
		writeError(w, http.StatusServiceUnavailable, "weixin integration not configured")
		return
	}
	userID, ok := requireUserID(w, r)
	if !ok {
		return
	}
	var request RedeemWeixinBindingTokenRequest
	decoder := json.NewDecoder(http.MaxBytesReader(w, r.Body, weixinBodyLimit))
	if err := decoder.Decode(&request); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	if strings.TrimSpace(request.Token) == "" {
		writeError(w, http.StatusBadRequest, "token is required")
		return
	}
	actorID, ok := parseUUIDOrBadRequest(w, userID, "user id")
	if !ok {
		return
	}
	service := weixin.NewBindingTokenService(h.Queries, h.TxStarter)
	redeemed, err := service.RedeemAndBind(r.Context(), request.Token, actorID)
	if err != nil {
		switch {
		case errors.Is(err, weixin.ErrBindingTokenInvalid):
			writeError(w, http.StatusGone, "binding token invalid or expired")
		case errors.Is(err, weixin.ErrBindingAlreadyAssigned):
			writeError(w, http.StatusConflict, "this Weixin account is already bound to a different Patchbay user")
		case errors.Is(err, weixin.ErrBindingNotWorkspaceMember):
			writeError(w, http.StatusForbidden, "binding refused (are you a workspace member?)")
		default:
			writeError(w, http.StatusInternalServerError, "failed to redeem token")
		}
		return
	}
	writeJSON(w, http.StatusOK, RedeemWeixinBindingTokenResponse{
		WorkspaceID: uuidToString(redeemed.WorkspaceID), InstallationID: uuidToString(redeemed.InstallationID), WeixinUserID: redeemed.WeixinUserID,
	})
}
