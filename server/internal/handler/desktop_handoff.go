package handler

import (
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"net/http"
	"regexp"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/auth"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

const desktopAuthCallbackProtocol = "patchbay"

var (
	desktopHandoffOpaquePattern = regexp.MustCompile(`^[A-Za-z0-9_-]{43}$`)
	desktopHandoffCodePattern   = regexp.MustCompile(`^pbd_[A-Za-z0-9_-]{43}$`)
)

type desktopAuthHandoffRequest struct {
	State            string `json:"state"`
	CodeChallenge    string `json:"code_challenge"`
	CallbackProtocol string `json:"callback_protocol"`
}

type desktopAuthHandoffRedeemRequest struct {
	Code         string `json:"code"`
	CodeVerifier string `json:"code_verifier"`
}

// desktopHandoffCodeChallenge derives the S256 PKCE challenge used by the
// Electron renderer. It is deliberately kept as a pure helper so the server
// and protocol tests can verify the exact wire representation.
func desktopHandoffCodeChallenge(verifier string) string {
	digest := sha256.Sum256([]byte(verifier))
	return base64.RawURLEncoding.EncodeToString(digest[:])
}

func generateDesktopHandoffCode() (string, error) {
	bytes := make([]byte, 32)
	if _, err := rand.Read(bytes); err != nil {
		return "", err
	}
	return "pbd_" + base64.RawURLEncoding.EncodeToString(bytes), nil
}

func validateDesktopHandoffRequest(req desktopAuthHandoffRequest) bool {
	return desktopHandoffOpaquePattern.MatchString(req.State) &&
		desktopHandoffOpaquePattern.MatchString(req.CodeChallenge) &&
		req.CallbackProtocol == desktopAuthCallbackProtocol
}

// requireFormalDesktopAuthActor is a handler-level backstop for the
// authenticated completion leg. Auth stamps X-Guest-User only after it has
// verified the pbg_ session, while client-supplied values are removed there.
// A guest session must not be able to turn its guest identity into the native
// JWT returned by the redeem leg. Keep this check local to Desktop handoff:
// Guest lifecycle endpoints intentionally continue to accept guest bearers.
func requireFormalDesktopAuthActor(w http.ResponseWriter, r *http.Request) bool {
	if isMachineCredentialActor(r) || r.Header.Get("X-Guest-User") == "true" {
		writeError(w, http.StatusForbidden, "desktop auth handoff requires a formal user")
		return false
	}
	return true
}

func (h *Handler) InitiateDesktopAuthHandoff(w http.ResponseWriter, r *http.Request) {
	var req desktopAuthHandoffRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil || !validateDesktopHandoffRequest(req) {
		writeError(w, http.StatusBadRequest, "invalid desktop auth handoff")
		return
	}

	if err := h.Queries.CreateDesktopAuthHandoff(r.Context(), db.CreateDesktopAuthHandoffParams{
		State:            req.State,
		CodeChallenge:    req.CodeChallenge,
		CallbackProtocol: req.CallbackProtocol,
	}); err != nil {
		if isUniqueViolation(err) {
			// A state collision must never overwrite an existing verifier binding.
			writeError(w, http.StatusConflict, "desktop auth handoff already exists")
			return
		}
		writeError(w, http.StatusInternalServerError, "failed to register desktop auth handoff")
		return
	}

	writeJSON(w, http.StatusOK, map[string]bool{"registered": true})
}

func (h *Handler) CompleteDesktopAuthHandoff(w http.ResponseWriter, r *http.Request) {
	if !requireFormalDesktopAuthActor(w, r) {
		return
	}
	userID, ok := requireUserID(w, r)
	if !ok {
		return
	}
	parsedUserID, err := util.ParseUUID(userID)
	if err != nil {
		writeError(w, http.StatusUnauthorized, "invalid authenticated user")
		return
	}

	var req desktopAuthHandoffRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil || !validateDesktopHandoffRequest(req) {
		writeError(w, http.StatusBadRequest, "invalid desktop auth handoff")
		return
	}

	code, err := generateDesktopHandoffCode()
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to create desktop auth handoff")
		return
	}

	protocol, err := h.Queries.CompleteDesktopAuthHandoff(r.Context(), db.CompleteDesktopAuthHandoffParams{
		State:         req.State,
		UserID:        parsedUserID,
		CodeHash:      pgtype.Text{String: auth.HashToken(code), Valid: true},
		CodeChallenge: req.CodeChallenge,
	})
	if err != nil {
		if isNotFound(err) {
			writeError(w, http.StatusGone, "desktop auth handoff is invalid or expired")
			return
		}
		writeError(w, http.StatusInternalServerError, "failed to complete desktop auth handoff")
		return
	}

	// The raw code is returned only to the already-authenticated browser. It is
	// never logged and is useless without the verifier held by the desktop app.
	writeJSON(w, http.StatusOK, map[string]string{
		"callback_protocol": protocol,
		"code":              code,
		"state":             req.State,
	})
}

func (h *Handler) RedeemDesktopAuthHandoff(w http.ResponseWriter, r *http.Request) {
	var req desktopAuthHandoffRedeemRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil ||
		!desktopHandoffCodePattern.MatchString(req.Code) ||
		!desktopHandoffOpaquePattern.MatchString(req.CodeVerifier) {
		writeError(w, http.StatusUnauthorized, "invalid desktop auth handoff")
		return
	}

	userID, err := h.Queries.RedeemDesktopAuthHandoff(r.Context(), db.RedeemDesktopAuthHandoffParams{
		CodeHash:      pgtype.Text{String: auth.HashToken(req.Code), Valid: true},
		CodeChallenge: desktopHandoffCodeChallenge(req.CodeVerifier),
	})
	if err != nil {
		if isNotFound(err) {
			// The DELETE query is atomic and consumes a valid handoff exactly once.
			writeError(w, http.StatusUnauthorized, "invalid desktop auth handoff")
			return
		}
		writeError(w, http.StatusInternalServerError, "failed to redeem desktop auth handoff")
		return
	}
	if !userID.Valid {
		writeError(w, http.StatusUnauthorized, "invalid desktop auth handoff")
		return
	}

	user, err := h.Queries.GetUser(r.Context(), userID)
	if err != nil {
		if errors.Is(err, auth.ErrTemporarilyDisabledUser) {
			writeError(w, http.StatusForbidden, auth.TemporarilyDisabledUserError)
			return
		}
		writeError(w, http.StatusUnauthorized, "invalid desktop auth handoff")
		return
	}
	token, err := h.issueJWT(user)
	if err != nil {
		if errors.Is(err, auth.ErrTemporarilyDisabledUser) {
			writeError(w, http.StatusForbidden, auth.TemporarilyDisabledUserError)
			return
		}
		writeError(w, http.StatusInternalServerError, "failed to generate token")
		return
	}

	writeJSON(w, http.StatusOK, map[string]string{"token": token})
}
