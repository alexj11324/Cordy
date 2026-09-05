package handler

import (
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"regexp"
	"strings"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/analytics"
	"github.com/patchbay-ai/patchbay/server/internal/auth"
	obsmetrics "github.com/patchbay-ai/patchbay/server/internal/metrics"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

const (
	desktopBrokerAuthHeader = "X-Patchbay-Desktop-Broker-Auth"
	authContractHeader      = "X-Patchbay-Auth-Contract-Version"
	authContractVersion     = "1"
)

var (
	desktopHandoffOpaquePattern    = regexp.MustCompile(`^[A-Za-z0-9._~-]{43,128}$`)
	desktopHandoffCodePattern      = regexp.MustCompile(`^pbd_[A-Za-z0-9_-]{43}$`)
	desktopBrokerSecretPattern     = regexp.MustCompile(`^[a-f0-9]{64}$`)
	desktopCallbackProtocolPattern = regexp.MustCompile(`^(?:patchbay|patchbay-canary-[a-f0-9]{16})$`)
)

type desktopAuthHandoffRequest struct {
	State            string `json:"state"`
	CodeChallenge    string `json:"code_challenge"`
	CallbackProtocol string `json:"callback_protocol"`
}

type desktopAuthHandoffRedeemRequest struct {
	State        string `json:"state,omitempty"`
	Code         string `json:"code"`
	CodeVerifier string `json:"code_verifier"`
}

type desktopGoogleAttemptRequest struct {
	Local         bool   `json:"local,omitempty"`
	State         string `json:"state"`
	CodeChallenge string `json:"code_challenge"`
}

func RequireDesktopBrokerAuth(expected string) func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			values := r.Header.Values(desktopBrokerAuthHeader)
			r.Header.Del(desktopBrokerAuthHeader)
			if !desktopBrokerSecretPattern.MatchString(expected) {
				writeError(w, http.StatusServiceUnavailable, "auth broker credential is not configured")
				return
			}
			if len(values) != 1 || len(values[0]) != len(expected) || subtle.ConstantTimeCompare([]byte(values[0]), []byte(expected)) != 1 {
				writeError(w, http.StatusForbidden, "invalid auth broker credential")
				return
			}
			next.ServeHTTP(w, r)
		})
	}
}

func requireAuthContract(w http.ResponseWriter, r *http.Request) bool {
	w.Header().Set(authContractHeader, authContractVersion)
	w.Header().Set("Cache-Control", "no-store")
	versions := r.Header.Values(authContractHeader)
	if len(versions) != 1 || versions[0] != authContractVersion {
		writeError(w, http.StatusConflict, "auth contract version rejected")
		return false
	}
	return true
}

func decodeDesktopGoogleAttempt(w http.ResponseWriter, r *http.Request, dst *desktopGoogleAttemptRequest) bool {
	decoder := json.NewDecoder(http.MaxBytesReader(w, r.Body, 4096))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(dst); err != nil {
		return false
	}
	return errors.Is(decoder.Decode(&struct{}{}), io.EOF)
}

func validDesktopGoogleAttempt(req desktopGoogleAttemptRequest) bool {
	return desktopHandoffOpaquePattern.MatchString(req.State) && desktopHandoffOpaquePattern.MatchString(req.CodeChallenge)
}

func (h *Handler) RegisterDesktopGoogleAttempt(w http.ResponseWriter, r *http.Request) {
	if !requireAuthContract(w, r) {
		return
	}
	var req desktopGoogleAttemptRequest
	if !decodeDesktopGoogleAttempt(w, r, &req) || !validDesktopGoogleAttempt(req) {
		writeError(w, http.StatusBadRequest, "invalid desktop Google OAuth binding")
		return
	}
	if _, err := h.Queries.RegisterDesktopGoogleAttempt(r.Context(), db.RegisterDesktopGoogleAttemptParams{State: req.State, CodeChallenge: req.CodeChallenge}); err != nil {
		if isNotFound(err) {
			writeError(w, http.StatusConflict, "desktop Google OAuth binding is already in use")
			return
		}
		writeError(w, http.StatusServiceUnavailable, "desktop Google OAuth is temporarily unavailable")
		return
	}
	writeJSON(w, http.StatusOK, map[string]bool{"registered": true})
}

func (h *Handler) finishDesktopGoogleAttempt(r *http.Request, token string, req desktopGoogleAttemptRequest) (string, string, int, string) {
	if h.ClerkAuth == nil {
		return "", "", http.StatusServiceUnavailable, "Clerk login is not configured"
	}
	startedAt, err := h.Queries.GetDesktopGoogleAttempt(r.Context(), db.GetDesktopGoogleAttemptParams{State: req.State, CodeChallenge: req.CodeChallenge})
	if err != nil || !startedAt.Valid {
		return "", "", http.StatusConflict, "fresh authentication is required"
	}
	identity, err := h.ClerkAuth.VerifyFreshSession(r.Context(), token, startedAt.Time)
	if err != nil {
		if errors.Is(err, errClerkUnavailable) {
			return "", "", http.StatusServiceUnavailable, "Clerk login is temporarily unavailable"
		}
		return "", "", http.StatusConflict, "fresh authentication is required"
	}
	user, isNew, err := h.findOrCreateUser(r.Context(), identity.Email)
	if err != nil {
		return "", "", http.StatusForbidden, "login rejected"
	}
	if isNew {
		evt := analytics.Signup(uuidToString(user.ID), user.Email, "")
		evt.Properties["auth_method"] = "clerk"
		obsmetrics.RecordEvent(h.Analytics, h.Metrics, evt)
	}
	if (identity.Name != "" && user.Name == strings.Split(identity.Email, "@")[0]) || (identity.AvatarURL != "" && !user.AvatarUrl.Valid) {
		name := user.Name
		if identity.Name != "" && user.Name == strings.Split(identity.Email, "@")[0] {
			name = identity.Name
		}
		avatar := user.AvatarUrl
		if identity.AvatarURL != "" && !user.AvatarUrl.Valid {
			avatar = pgtype.Text{String: identity.AvatarURL, Valid: true}
		}
		if updated, updateErr := h.Queries.UpdateUser(r.Context(), db.UpdateUserParams{ID: user.ID, Name: name, AvatarUrl: avatar}); updateErr == nil {
			user = updated
		}
	}
	code, err := generateDesktopHandoffCode()
	if err != nil {
		return "", "", http.StatusInternalServerError, "failed to create desktop auth handoff"
	}
	// Hash the complete purpose-prefixed code. Local identity grants can never
	// be changed into production session grants by replacing their prefix.
	if req.Local {
		code = "pbl_" + strings.TrimPrefix(code, "pbd_")
	}
	protocol, err := h.Queries.CompleteDesktopAuthHandoff(r.Context(), db.CompleteDesktopAuthHandoffParams{State: req.State, UserID: user.ID, CodeHash: pgtype.Text{String: auth.HashToken(code), Valid: true}, CodeChallenge: req.CodeChallenge})
	if err != nil {
		return "", "", http.StatusConflict, "desktop Google OAuth attempt was already used"
	}
	return protocol, code, 0, ""
}

func (h *Handler) CompleteDesktopGoogleAttempt(w http.ResponseWriter, r *http.Request) {
	if !requireAuthContract(w, r) {
		return
	}
	var req desktopGoogleAttemptRequest
	if !decodeDesktopGoogleAttempt(w, r, &req) || !validDesktopGoogleAttempt(req) {
		writeError(w, http.StatusBadRequest, "invalid desktop Google OAuth binding")
		return
	}
	authorization := r.Header.Get("Authorization")
	if !strings.HasPrefix(authorization, "Bearer ") || len(authorization) > 8192 || strings.ContainsAny(authorization, "\r\n") {
		writeError(w, http.StatusUnauthorized, "Clerk session is required")
		return
	}
	protocol, code, status, errMsg := h.finishDesktopGoogleAttempt(r, strings.TrimSpace(strings.TrimPrefix(authorization, "Bearer ")), req)
	if errMsg != "" {
		writeError(w, status, errMsg)
		return
	}
	writeJSON(w, http.StatusOK, map[string]string{"callback_protocol": protocol, "code": code})
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

func validateDesktopHandoffInitiate(req desktopAuthHandoffRequest) bool {
	return validDesktopHandoffBinding(req.State, req.CodeChallenge) &&
		desktopCallbackProtocolPattern.MatchString(req.CallbackProtocol)
}

func validateDesktopHandoffComplete(req desktopAuthHandoffRequest) bool {
	// The browser must not choose the OS handler. Complete is bound by PKCE
	// state + challenge; the callback scheme is the one stored at initiate.
	return validDesktopHandoffBinding(req.State, req.CodeChallenge)
}

func validDesktopHandoffBinding(state, codeChallenge string) bool {
	return desktopHandoffOpaquePattern.MatchString(state) &&
		desktopHandoffOpaquePattern.MatchString(codeChallenge)
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
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil || !validateDesktopHandoffInitiate(req) {
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
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil || !validateDesktopHandoffComplete(req) {
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
	w.Header().Set("Cache-Control", "no-store")
	var req desktopAuthHandoffRedeemRequest
	if !decodeDesktopHandoffRedeem(w, r, &req) {
		writeError(w, http.StatusUnauthorized, "invalid desktop auth handoff")
		return
	}
	if desktopLocalIdentityCodePattern.MatchString(req.Code) {
		h.redeemLocalDesktopSession(w, r, req)
		return
	}
	if !desktopHandoffCodePattern.MatchString(req.Code) {
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
