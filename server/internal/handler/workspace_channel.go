package handler

import (
	"encoding/json"
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/internal/util"
)

func (h *Handler) ListWorkspaceChannels(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	if wsID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return
	}
	wsUUID, _ := util.ParseUUID(wsID)
	chs, err := h.Queries.ListWorkspaceChannels(r.Context(), wsUUID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"channels": chs})
}

func (h *Handler) CreateWorkspaceChannel(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	if wsID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return
	}
	var body struct {
		Slug   string  `json:"slug"`
		Name   *string `json:"name"`
		Status *string `json:"status"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil || body.Slug == "" {
		writeError(w, http.StatusBadRequest, "invalid json or missing slug")
		return
	}
	wsUUID, _ := util.ParseUUID(wsID)
	ch, err := h.Queries.CreateWorkspaceChannel(r.Context(), db.CreateWorkspaceChannelParams{
		WorkspaceID: wsUUID,
		Slug:        body.Slug,
		Column3:     body.Name,
		Status:      pgtype.Text{String: *body.Status, Valid: body.Status != nil},
	})
	if err != nil {
		if isUniqueViolation(err) || isCheckViolation(err) {
			writeErrorCode(w, http.StatusConflict, "channel_conflict", err.Error())
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	writeJSON(w, http.StatusCreated, ch)
}

func (h *Handler) GetWorkspaceChannel(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	if wsID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return
	}
	id := chi.URLParam(r, "id")
	cid, ok := parseUUIDOrBadRequest(w, id, "channel id")
	if !ok {
		return
	}
	wsUUID, _ := util.ParseUUID(wsID)
	ch, err := h.Queries.GetWorkspaceChannelByID(r.Context(), db.GetWorkspaceChannelByIDParams{ID: cid, WorkspaceID: wsUUID})
	if err != nil {
		writeErrorCode(w, http.StatusNotFound, "not_found", "not found")
		return
	}
	writeJSON(w, http.StatusOK, ch)
}

func (h *Handler) ListWorkspaceChannelMessages(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	if wsID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return
	}
	channelID := chi.URLParam(r, "id")
	cid, ok := parseUUIDOrBadRequest(w, channelID, "channel id")
	if !ok {
		return
	}
	wsUUID, _ := util.ParseUUID(wsID)
	msgs, err := h.Queries.ListWorkspaceChannelMessages(r.Context(), db.ListWorkspaceChannelMessagesParams{WorkspaceID: wsUUID, ChannelID: cid, Limit: 64, Offset: 0})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"messages": msgs})
}

func (h *Handler) CreateWorkspaceChannelMessage(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	if wsID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return
	}
	channelID := chi.URLParam(r, "id")
	cid, ok := parseUUIDOrBadRequest(w, channelID, "channel id")
	if !ok {
		return
	}
	var body struct {
		AuthorType string  `json:"author_type"`
		AuthorID   string  `json:"author_id"`
		Content    string  `json:"content"`
		ParentID   *string `json:"parent_id"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil || body.Content == "" || body.AuthorType == "" || body.AuthorID == "" {
		writeError(w, http.StatusBadRequest, "invalid json or missing content/author")
		return
	}
	aid, ok := parseUUIDOrBadRequest(w, body.AuthorID, "author_id")
	if !ok {
		return
	}
	wsUUID, _ := util.ParseUUID(wsID)
	arg := db.CreateWorkspaceChannelMessageParams{WorkspaceID: wsUUID, ChannelID: cid, AuthorType: body.AuthorType, AuthorID: aid, Content: body.Content}
	if body.ParentID != nil && *body.ParentID != "" {
		u, _ := util.ParseUUID(*body.ParentID)
		arg.ParentID = pgtype.UUID{Bytes: u.Bytes, Valid: true}
	}
	msg, err := h.Queries.CreateWorkspaceChannelMessage(r.Context(), arg)
	if err != nil {
		if isCheckViolation(err) {
			writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_message", err.Error())
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	writeJSON(w, http.StatusCreated, msg)
}
