package handler

import (
	crand "crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strings"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
)

const (
	guestTokenPrefix    = "pbg_"
	guestTokenHexLength = 40
	guestTokenLength    = len(guestTokenPrefix) + guestTokenHexLength
	guestJSONBodyLimit  = 4 << 10

	guestSessionActive  = "active"
	guestSessionClaimed = "claimed"
	guestSessionRevoked = "revoked"
)

// guestSessionResponse is the public representation of a guest session.
// token_hash is deliberately excluded: although the database stores only a
// hash, returning it still gives callers a reusable credential verifier and
// makes future schema additions easy to leak accidentally.
type guestSessionResponse struct {
	ID        pgtype.UUID        `json:"id"`
	UserID    pgtype.UUID        `json:"user_id"`
	Status    string             `json:"status"`
	CreatedAt pgtype.Timestamptz `json:"created_at"`
	ClaimedAt pgtype.Timestamptz `json:"claimed_at"`
	ClaimedBy pgtype.UUID        `json:"claimed_by"`
}

func hashGuestToken(raw string) string {
	h := sha256.Sum256([]byte(raw))
	return hex.EncodeToString(h[:])
}

func validGuestToken(raw string) bool {
	if len(raw) != guestTokenLength || !strings.HasPrefix(raw, guestTokenPrefix) {
		return false
	}
	_, err := hex.DecodeString(raw[len(guestTokenPrefix):])
	return err == nil
}

func generateGuestToken() (string, error) {
	raw := make([]byte, guestTokenHexLength/2)
	if _, err := crand.Read(raw); err != nil {
		return "", err
	}
	return guestTokenPrefix + hex.EncodeToString(raw), nil
}

func guestSessionWire(sess db.GuestSession) guestSessionResponse {
	return guestSessionResponse{
		ID:        sess.ID,
		UserID:    sess.UserID,
		Status:    sess.Status,
		CreatedAt: sess.CreatedAt,
		ClaimedAt: sess.ClaimedAt,
		ClaimedBy: sess.ClaimedBy,
	}
}

// decodeGuestJSON keeps the small auth surface bounded and rejects trailing
// JSON/unknown fields. In particular, callers cannot smuggle an alternate
// identity field through a silently ignored payload.
func decodeGuestJSON(w http.ResponseWriter, r *http.Request, dst any) bool {
	r.Body = http.MaxBytesReader(w, r.Body, guestJSONBodyLimit)
	decoder := json.NewDecoder(r.Body)
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(dst); err != nil {
		writeError(w, http.StatusBadRequest, "invalid json")
		return false
	}
	var extra any
	if err := decoder.Decode(&extra); err != io.EOF {
		writeError(w, http.StatusBadRequest, "invalid json")
		return false
	}
	return true
}

// requireGuestSessionCaller resolves the server-authenticated user and
// rejects machine credentials. The guest endpoints are account lifecycle
// operations; accepting a task token or cloud PAT here would let an agent
// create, inspect, claim, or revoke another account's session.
func (h *Handler) requireGuestSessionCaller(w http.ResponseWriter, r *http.Request) (db.User, bool) {
	if isMachineCredentialActor(r) {
		writeError(w, http.StatusForbidden, "guest sessions require a human actor")
		return db.User{}, false
	}
	if h == nil || h.Queries == nil {
		writeError(w, http.StatusServiceUnavailable, "guest session unavailable")
		return db.User{}, false
	}

	rawUserID := strings.TrimSpace(requestUserID(r))
	if rawUserID == "" {
		writeErrorCode(w, http.StatusUnauthorized, "login_required", "user not authenticated")
		return db.User{}, false
	}
	userID, err := util.ParseUUID(rawUserID)
	if err != nil {
		writeErrorCode(w, http.StatusUnauthorized, "login_required", "user not authenticated")
		return db.User{}, false
	}
	caller, err := h.Queries.GetUser(r.Context(), userID)
	if err != nil {
		if isNotFound(err) {
			writeErrorCode(w, http.StatusUnauthorized, "login_required", "user not authenticated")
			return db.User{}, false
		}
		writeError(w, http.StatusServiceUnavailable, "user status unavailable")
		return db.User{}, false
	}
	if !caller.ID.Valid || caller.ID != userID {
		writeErrorCode(w, http.StatusUnauthorized, "login_required", "user not authenticated")
		return db.User{}, false
	}
	return caller, true
}

func guestSessionBelongsTo(sess db.GuestSession, user db.User) bool {
	return sess.ID.Valid && sess.UserID.Valid && user.ID.Valid && sess.UserID == user.ID
}

// guestSessionIsActive is deliberately status-based. The adopted schema has
// no expiry column or documented lifetime, so inventing a wall-clock TTL here
// would make the Go surface disagree with the persisted contract. Claimed and
// revoked are terminal states and are not consumable bearer sessions.
func guestSessionIsActive(sess db.GuestSession) bool {
	return sess.ID.Valid && sess.UserID.Valid && sess.Status == guestSessionActive
}

// guestTokenMatches is a defense-in-depth proof check after the indexed token
// lookup. The lookup already uses the SHA-256 hash, but retaining the explicit
// constant-time comparison keeps the ownership/possession invariant intact if
// the query later gains joins or a broader selection predicate.
func guestTokenMatches(sess db.GuestSession, rawToken string) bool {
	want := hashGuestToken(rawToken)
	return subtle.ConstantTimeCompare([]byte(sess.TokenHash), []byte(want)) == 1
}

// loadOwnedGuestSession centralizes the owner and guest-identity checks for
// reads and revocation. A session row pointing at a formal user is not a
// usable guest session, even if an old or manually-created row still exists.
func (h *Handler) loadOwnedGuestSession(w http.ResponseWriter, r *http.Request, caller db.User, id pgtype.UUID) (db.GuestSession, bool) {
	if h == nil || h.Queries == nil {
		writeError(w, http.StatusServiceUnavailable, "guest session unavailable")
		return db.GuestSession{}, false
	}
	sess, err := h.Queries.GetGuestSessionByID(r.Context(), id)
	if err != nil {
		if !isNotFound(err) {
			writeError(w, http.StatusServiceUnavailable, "guest session unavailable")
			return db.GuestSession{}, false
		}
		writeErrorCode(w, http.StatusNotFound, "not_found", "not found")
		return db.GuestSession{}, false
	}
	if !guestSessionBelongsTo(sess, caller) {
		// Do not disclose whether another account's UUID exists.
		writeErrorCode(w, http.StatusNotFound, "not_found", "not found")
		return db.GuestSession{}, false
	}
	owner, err := h.Queries.GetUser(r.Context(), sess.UserID)
	if err != nil {
		if !isNotFound(err) {
			writeError(w, http.StatusServiceUnavailable, "guest session unavailable")
			return db.GuestSession{}, false
		}
		writeErrorCode(w, http.StatusNotFound, "not_found", "not found")
		return db.GuestSession{}, false
	}
	if !owner.IsGuest {
		writeErrorCode(w, http.StatusNotFound, "not_found", "not found")
		return db.GuestSession{}, false
	}
	return sess, true
}

// loadGuestSessionByToken proves possession of the opaque token and keeps
// status checks in the handler because the query is intentionally able to
// distinguish active, claimed, and revoked rows for lifecycle responses.
func (h *Handler) loadGuestSessionByToken(r *http.Request, rawToken string) (db.GuestSession, error) {
	if h == nil || h.Queries == nil {
		return db.GuestSession{}, errors.New("guest session queries unavailable")
	}
	return h.Queries.GetGuestSessionByTokenHash(r.Context(), hashGuestToken(rawToken))
}

// CreateGuestAuth is the public guest-entry transaction for the formal
// /auth/guest contract. It intentionally does not accept a caller or a user
// id: the server creates both the guest user and its opaque bearer in one
// transaction. The router exposes this method at /auth/guest and the auth
// middleware recognizes the resulting pbg_ bearer on subsequent requests.
func (h *Handler) CreateGuestAuth(w http.ResponseWriter, r *http.Request) {
	if h == nil || h.TxStarter == nil {
		writeError(w, http.StatusServiceUnavailable, "guest session unavailable")
		return
	}

	rawToken, err := generateGuestToken()
	if err != nil {
		writeError(w, http.StatusServiceUnavailable, "guest session unavailable")
		return
	}
	userID := dbid.NewV7()
	sessionID := dbid.NewV7()
	email := fmt.Sprintf("guest+%s@guest.patchbay.invalid", util.UUIDToString(userID))

	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		writeError(w, http.StatusServiceUnavailable, "guest session unavailable")
		return
	}
	defer func() { _ = tx.Rollback(r.Context()) }()

	var user db.User
	err = tx.QueryRow(r.Context(), `
		INSERT INTO "user" (id, name, email, is_guest)
		VALUES ($1, $2, $3, TRUE)
		RETURNING id, name, email, avatar_url, created_at, updated_at,
		          onboarded_at, onboarding_questionnaire, cloud_waitlist_email,
		          cloud_waitlist_reason, starter_content_state, language,
		          profile_description, timezone, is_guest
	`, userID, "Guest", email).Scan(
		&user.ID,
		&user.Name,
		&user.Email,
		&user.AvatarUrl,
		&user.CreatedAt,
		&user.UpdatedAt,
		&user.OnboardedAt,
		&user.OnboardingQuestionnaire,
		&user.CloudWaitlistEmail,
		&user.CloudWaitlistReason,
		&user.StarterContentState,
		&user.Language,
		&user.ProfileDescription,
		&user.Timezone,
		&user.IsGuest,
	)
	if err != nil {
		writeError(w, http.StatusServiceUnavailable, "guest session unavailable")
		return
	}
	if _, err := tx.Exec(r.Context(), `
		INSERT INTO guest_session (id, user_id, token_hash, status)
		VALUES ($1, $2, $3, $4)
	`, sessionID, userID, hashGuestToken(rawToken), guestSessionActive); err != nil {
		writeError(w, http.StatusServiceUnavailable, "guest session unavailable")
		return
	}
	if err := tx.Commit(r.Context()); err != nil {
		writeError(w, http.StatusServiceUnavailable, "guest session unavailable")
		return
	}
	if !user.IsGuest || user.ID != userID {
		writeError(w, http.StatusServiceUnavailable, "guest session unavailable")
		return
	}
	writeJSON(w, http.StatusOK, LoginResponse{
		Token: rawToken,
		User:  h.userToResponse(user),
	})
}

func (h *Handler) CreateGuestSession(w http.ResponseWriter, r *http.Request) {
	caller, ok := h.requireGuestSessionCaller(w, r)
	if !ok {
		return
	}

	var body struct {
		UserID string `json:"user_id"`
		Token  string `json:"token"`
	}
	if !decodeGuestJSON(w, r, &body) {
		return
	}

	// The caller is the only permitted owner. The optional body field is kept
	// for the existing wire shape, but it can never select another tenant or
	// user. A guest session may not be attached to a formal account.
	userID := caller.ID
	if strings.TrimSpace(body.UserID) != "" {
		requestedUserID, valid := parseUUIDOrBadRequest(w, strings.TrimSpace(body.UserID), "user_id")
		if !valid {
			return
		}
		if requestedUserID != caller.ID {
			writeError(w, http.StatusForbidden, "guest session owner mismatch")
			return
		}
		userID = requestedUserID
	}
	if !caller.IsGuest {
		writeErrorCode(w, http.StatusForbidden, "formal_login_required", "guest sessions require a guest user")
		return
	}

	rawToken := strings.TrimSpace(body.Token)
	if !validGuestToken(rawToken) {
		writeError(w, http.StatusBadRequest, "token must be a pbg_ token")
		return
	}
	sess, err := h.Queries.CreateGuestSession(r.Context(), db.CreateGuestSessionParams{
		UserID:    userID,
		TokenHash: hashGuestToken(rawToken),
		Status:    pgtype.Text{String: guestSessionActive, Valid: true},
		ID:        dbid.NewV7(),
	})
	if err != nil {
		if isUniqueViolation(err) {
			writeErrorCode(w, http.StatusConflict, "guest_conflict", "guest session already exists")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	writeJSON(w, http.StatusCreated, guestSessionWire(sess))
}

func (h *Handler) GetGuestSession(w http.ResponseWriter, r *http.Request) {
	caller, ok := h.requireGuestSessionCaller(w, r)
	if !ok {
		return
	}

	id := chi.URLParam(r, "id")
	gid, ok := parseUUIDOrBadRequest(w, id, "guest session id")
	if !ok {
		return
	}
	sess, ok := h.loadOwnedGuestSession(w, r, caller, gid)
	if !ok {
		return
	}
	writeJSON(w, http.StatusOK, guestSessionWire(sess))
}

func (h *Handler) ClaimGuestSession(w http.ResponseWriter, r *http.Request) {
	caller, ok := h.requireGuestSessionCaller(w, r)
	if !ok {
		return
	}
	if caller.IsGuest {
		writeErrorCode(w, http.StatusForbidden, "formal_login_required", "formal login required")
		return
	}

	id := chi.URLParam(r, "id")
	gid, ok := parseUUIDOrBadRequest(w, id, "guest session id")
	if !ok {
		return
	}
	var body struct {
		ClaimedBy string `json:"claimed_by"`
		Token     string `json:"token"`
	}
	if !decodeGuestJSON(w, r, &body) {
		return
	}

	// claimed_by is retained for compatibility but is server-bound to the
	// authenticated formal user. Possession of the token is also required;
	// knowing a session UUID alone must not be enough to claim it.
	claimedBy := caller.ID
	if strings.TrimSpace(body.ClaimedBy) != "" {
		requestedClaimer, valid := parseUUIDOrBadRequest(w, strings.TrimSpace(body.ClaimedBy), "claimed_by")
		if !valid {
			return
		}
		if requestedClaimer != caller.ID {
			writeError(w, http.StatusForbidden, "claim owner mismatch")
			return
		}
		claimedBy = requestedClaimer
	}
	rawToken := strings.TrimSpace(body.Token)
	if !validGuestToken(rawToken) {
		writeError(w, http.StatusBadRequest, "token must be a pbg_ token")
		return
	}

	byToken, err := h.loadGuestSessionByToken(r, rawToken)
	if err != nil || byToken.ID != gid || !guestSessionIsActive(byToken) || !guestTokenMatches(byToken, rawToken) {
		if err != nil && !isNotFound(err) {
			writeError(w, http.StatusServiceUnavailable, "guest session unavailable")
			return
		}
		if byToken.ID == gid && byToken.ID.Valid && byToken.Status != guestSessionActive {
			writeErrorCode(w, http.StatusConflict, "guest_not_active", "guest session not active")
			return
		}
		writeErrorCode(w, http.StatusNotFound, "not_found", "not found")
		return
	}
	guestUser, err := h.Queries.GetUser(r.Context(), byToken.UserID)
	if err != nil || !guestUser.IsGuest {
		writeErrorCode(w, http.StatusNotFound, "not_found", "not found")
		return
	}

	sess, err := h.Queries.ClaimGuestSession(r.Context(), db.ClaimGuestSessionParams{
		ID:        gid,
		ClaimedBy: claimedBy,
	})
	if err != nil {
		if isNotFound(err) {
			// A concurrent claim/revoke won the status transition.
			writeErrorCode(w, http.StatusConflict, "guest_not_active", "guest session not active")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	if sess.Status != guestSessionClaimed || !sess.ID.Valid {
		writeErrorCode(w, http.StatusConflict, "guest_not_active", "guest session not active")
		return
	}
	writeJSON(w, http.StatusOK, guestSessionWire(sess))
}

// RevokeGuestSession is the owner-authenticated lifecycle endpoint for
// callers that expose revocation separately from logout. The router mounts
// this at /api/guest-sessions/{id}/revoke; keeping the handler proof-bound
// prevents a future route from accidentally inheriting the old id-only
// revocation.
func (h *Handler) RevokeGuestSession(w http.ResponseWriter, r *http.Request) {
	caller, ok := h.requireGuestSessionCaller(w, r)
	if !ok {
		return
	}

	id := chi.URLParam(r, "id")
	gid, ok := parseUUIDOrBadRequest(w, id, "guest session id")
	if !ok {
		return
	}
	var body struct {
		Token string `json:"token"`
	}
	if !decodeGuestJSON(w, r, &body) {
		return
	}
	rawToken := strings.TrimSpace(body.Token)
	if !validGuestToken(rawToken) {
		writeError(w, http.StatusBadRequest, "token must be a pbg_ token")
		return
	}

	sess, err := h.loadGuestSessionByToken(r, rawToken)
	if err != nil {
		if isNotFound(err) {
			writeErrorCode(w, http.StatusNotFound, "not_found", "not found")
			return
		}
		writeError(w, http.StatusServiceUnavailable, "guest session unavailable")
		return
	}
	if sess.ID != gid || !guestTokenMatches(sess, rawToken) {
		writeErrorCode(w, http.StatusNotFound, "not_found", "not found")
		return
	}
	owned, ok := h.loadOwnedGuestSession(w, r, caller, gid)
	if !ok {
		return
	}
	if !guestSessionIsActive(owned) {
		writeErrorCode(w, http.StatusConflict, "guest_not_active", "guest session not active")
		return
	}

	revoked, err := h.Queries.RevokeGuestSession(r.Context(), gid)
	if err != nil {
		if isNotFound(err) {
			writeErrorCode(w, http.StatusConflict, "guest_not_active", "guest session not active")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	if revoked.Status != guestSessionRevoked || !revoked.ID.Valid {
		writeErrorCode(w, http.StatusConflict, "guest_not_active", "guest session not active")
		return
	}
	writeJSON(w, http.StatusOK, guestSessionWire(revoked))
}
