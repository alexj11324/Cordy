package handler

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
)

func hashGuestToken(raw string) string {
	h := sha256.Sum256([]byte(raw))
	return hex.EncodeToString(h[:])
}

func (h *Handler) CreateGuestSession(w http.ResponseWriter, r *http.Request) {
	var body struct {
		UserID string `json:"user_id"`
		Token  string `json:"token"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil || body.UserID == "" || body.Token == "" {
		writeError(w, http.StatusBadRequest, "invalid json or missing user_id/token")
		return
	}
	uid, ok := parseUUIDOrBadRequest(w, body.UserID, "user_id")
	if !ok {
		return
	}
	hash := hashGuestToken(body.Token)
	sess, err := h.Queries.CreateGuestSession(r.Context(), db.CreateGuestSessionParams{
		UserID: uid, TokenHash: hash, ID: dbid.NewV7(),
	})
	if err != nil {
		if isUniqueViolation(err) {
			writeErrorCode(w, http.StatusConflict, "guest_conflict", err.Error())
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	writeJSON(w, http.StatusCreated, sess)
}

func (h *Handler) GetGuestSession(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")
	gid, ok := parseUUIDOrBadRequest(w, id, "guest session id")
	if !ok {
		return
	}
	sess, err := h.Queries.GetGuestSessionByID(r.Context(), gid)
	if err != nil {
		writeErrorCode(w, http.StatusNotFound, "not_found", "not found")
		return
	}
	writeJSON(w, http.StatusOK, sess)
}

func (h *Handler) ClaimGuestSession(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")
	gid, ok := parseUUIDOrBadRequest(w, id, "guest session id")
	if !ok {
		return
	}
	var body struct {
		ClaimedBy string `json:"claimed_by"`
	}
	_ = json.NewDecoder(r.Body).Decode(&body)
	var claimedBy pgtype.UUID
	if body.ClaimedBy != "" {
		u, _ := util.ParseUUID(body.ClaimedBy)
		claimedBy = u
	}
	sess, err := h.Queries.ClaimGuestSession(r.Context(), db.ClaimGuestSessionParams{ID: gid, ClaimedBy: claimedBy})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	if !sess.ID.Valid {
		writeErrorCode(w, http.StatusConflict, "guest_not_active", "guest session not active")
		return
	}
	writeJSON(w, http.StatusOK, sess)
}
