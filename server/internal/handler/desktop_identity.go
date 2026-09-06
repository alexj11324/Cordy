package handler

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"net/mail"
	"regexp"
	"strings"
	"time"

	chimw "github.com/go-chi/chi/v5/middleware"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/auth"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

var desktopLocalIdentityCodePattern = regexp.MustCompile(`^pbl_[A-Za-z0-9_-]{43}$`)
var errDesktopIdentityRejected = errors.New("desktop identity rejected")
var errDesktopIdentityUnavailable = errors.New("desktop identity unavailable")

type desktopIdentityResponse struct {
	Email     string `json:"email"`
	Name      string `json:"name"`
	AvatarURL string `json:"avatar_url"`
}

func decodeDesktopHandoffRedeem(w http.ResponseWriter, r *http.Request, dst *desktopAuthHandoffRedeemRequest) bool {
	decoder := json.NewDecoder(http.MaxBytesReader(w, r.Body, 4096))
	decoder.DisallowUnknownFields()
	return decoder.Decode(dst) == nil && errors.Is(decoder.Decode(&struct{}{}), io.EOF) &&
		desktopHandoffOpaquePattern.MatchString(dst.CodeVerifier)
}

// RedeemDesktopLocalIdentity consumes a local-only grant. This endpoint never
// issues a production bearer, and the normal session endpoint rejects pbl_.
// Native callbacks carry authorization codes, not bearer tokens (RFC 8252 §8.1).
func (h *Handler) RedeemDesktopLocalIdentity(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Cache-Control", "no-store")
	var req desktopAuthHandoffRedeemRequest
	if !decodeDesktopHandoffRedeem(w, r, &req) || !desktopLocalIdentityCodePattern.MatchString(req.Code) || !desktopHandoffOpaquePattern.MatchString(req.State) {
		writeError(w, http.StatusUnauthorized, "invalid desktop identity grant")
		return
	}
	userID, err := h.Queries.RedeemDesktopLocalIdentity(r.Context(), db.RedeemDesktopLocalIdentityParams{
		CodeHash: pgtype.Text{String: auth.HashToken(req.Code), Valid: true},
		State:    req.State, CodeChallenge: desktopHandoffCodeChallenge(req.CodeVerifier),
	})
	if err != nil || !userID.Valid {
		writeError(w, http.StatusUnauthorized, "invalid desktop identity grant")
		return
	}
	user, err := h.Queries.GetUser(r.Context(), userID)
	if err != nil || auth.IsTemporarilyDisabledUser(uuidToString(user.ID), user.Email) {
		writeError(w, http.StatusUnauthorized, "invalid desktop identity grant")
		return
	}
	writeJSON(w, http.StatusOK, desktopIdentityResponse{Email: user.Email, Name: user.Name, AvatarURL: user.AvatarUrl.String})
}

func redeemHostedDesktopIdentity(ctx context.Context, req desktopAuthHandoffRedeemRequest) (ClerkIdentity, error) {
	client := &http.Client{Timeout: 10 * time.Second, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	// The authority is fixed by this application, never taken from a browser URL.
	return requestDesktopIdentity(ctx, req, client, "https://api.aspectlylabs.com/api/desktop-identity/redeem")
}

func requestDesktopIdentity(ctx context.Context, binding desktopAuthHandoffRedeemRequest, client *http.Client, endpoint string) (ClerkIdentity, error) {
	body, err := json.Marshal(binding)
	if err != nil {
		return ClerkIdentity{}, errDesktopIdentityRejected
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, endpoint, bytes.NewReader(body))
	if err != nil {
		return ClerkIdentity{}, errDesktopIdentityRejected
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json")
	response, err := client.Do(req)
	if err != nil {
		slog.WarnContext(ctx, "desktop identity request failed", "stage", "transport", "request_id", chimw.GetReqID(ctx))
		return ClerkIdentity{}, errDesktopIdentityUnavailable
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusOK {
		slog.WarnContext(ctx, "desktop identity request failed", "stage", "upstream_response", "upstream_status", response.StatusCode, "request_id", chimw.GetReqID(ctx))
		if response.StatusCode == http.StatusUnauthorized {
			return ClerkIdentity{}, errDesktopIdentityRejected
		}
		return ClerkIdentity{}, errDesktopIdentityUnavailable
	}
	raw, err := io.ReadAll(io.LimitReader(response.Body, 8193))
	if err != nil || len(raw) > 8192 {
		slog.WarnContext(ctx, "desktop identity request failed", "stage", "response_body", "request_id", chimw.GetReqID(ctx))
		return ClerkIdentity{}, errDesktopIdentityUnavailable
	}
	var identity desktopIdentityResponse
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	if decoder.Decode(&identity) != nil || !errors.Is(decoder.Decode(&struct{}{}), io.EOF) {
		slog.WarnContext(ctx, "desktop identity request failed", "stage", "response_json", "request_id", chimw.GetReqID(ctx))
		return ClerkIdentity{}, errDesktopIdentityUnavailable
	}
	address, err := mail.ParseAddress(identity.Email)
	if err != nil || address.Address != identity.Email || len(identity.Email) > 320 || len(identity.Name) > 512 || len(identity.AvatarURL) > 2048 {
		slog.WarnContext(ctx, "desktop identity request failed", "stage", "identity_fields", "request_id", chimw.GetReqID(ctx))
		return ClerkIdentity{}, errDesktopIdentityUnavailable
	}
	return ClerkIdentity{Email: strings.ToLower(identity.Email), Name: identity.Name, AvatarURL: identity.AvatarURL}, nil
}

func (h *Handler) redeemLocalDesktopSession(w http.ResponseWriter, r *http.Request, req desktopAuthHandoffRedeemRequest) {
	w.Header().Set("Cache-Control", "no-store")
	if h.redeemDesktopIdentity == nil || !desktopHandoffOpaquePattern.MatchString(req.State) {
		writeError(w, http.StatusUnauthorized, "invalid desktop identity grant")
		return
	}
	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		writeError(w, http.StatusServiceUnavailable, "desktop login unavailable")
		return
	}
	defer tx.Rollback(r.Context())
	queries := h.Queries.WithTx(tx)
	// Claim the locally initiated binding before contacting the identity authority.
	// Concurrent redemptions serialize here; user creation and consumption commit together.
	consumed, err := queries.ConsumeDesktopLocalAuthAttempt(r.Context(), db.ConsumeDesktopLocalAuthAttemptParams{State: req.State, CodeChallenge: desktopHandoffCodeChallenge(req.CodeVerifier)})
	if err != nil || consumed != 1 {
		slog.WarnContext(r.Context(), "desktop login rejected", "stage", "local_binding", "database_error", err != nil, "request_id", chimw.GetReqID(r.Context()))
		writeError(w, http.StatusUnauthorized, "invalid desktop identity grant")
		return
	}
	identity, err := h.redeemDesktopIdentity(r.Context(), req)
	if err != nil {
		if errors.Is(err, errDesktopIdentityUnavailable) {
			writeError(w, http.StatusBadGateway, "desktop login temporarily unavailable; sign in again")
			return
		}
		slog.WarnContext(r.Context(), "desktop login rejected", "stage", "identity_grant", "request_id", chimw.GetReqID(r.Context()))
		// A consumed remote grant cannot be recovered safely; start a fresh login.
		writeError(w, http.StatusUnauthorized, "desktop login expired; sign in again")
		return
	}
	user, _, err := h.findOrCreateUserWithQueries(r.Context(), queries, identity.Email)
	if err != nil {
		writeError(w, http.StatusForbidden, "login rejected")
		return
	}
	if identity.Name != "" || identity.AvatarURL != "" {
		name, avatar := user.Name, user.AvatarUrl
		if identity.Name != "" && name == strings.Split(user.Email, "@")[0] {
			name = identity.Name
		}
		if identity.AvatarURL != "" && !avatar.Valid {
			avatar = pgtype.Text{String: identity.AvatarURL, Valid: true}
		}
		user, err = queries.UpdateUser(r.Context(), db.UpdateUserParams{ID: user.ID, Name: name, AvatarUrl: avatar})
		if err != nil {
			writeError(w, http.StatusInternalServerError, "failed to create desktop session")
			return
		}
	}
	token, err := h.issueJWT(user)
	if err != nil {
		writeError(w, http.StatusForbidden, "login rejected")
		return
	}
	if err := tx.Commit(r.Context()); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to create desktop session")
		return
	}
	slog.InfoContext(r.Context(), "desktop login completed", "stage", "local_session_committed", "request_id", chimw.GetReqID(r.Context()))
	writeJSON(w, http.StatusOK, map[string]string{"token": token})
}
