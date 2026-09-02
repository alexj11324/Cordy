package handler

import (
	"encoding/json"
	"errors"
	"net/http"
	"strings"
	"time"

	"github.com/go-chi/chi/v5"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/slack"
)

// ManagedSlackOAuthCallbackPath is the public OAuth callback Slack redirects
// the installer's browser to. It is public (no workspace auth): Slack's
// browser redirect carries no Patchbay session, so the workspace + installer
// are recovered from the single-use state token instead. The redirect_uri sent
// to Slack is always this path joined onto the configured public API base URL.
const ManagedSlackOAuthCallbackPath = "/api/integrations/slack/oauth/callback"

// BeginManagedSlackInstallRequest carries the post-install destination: after
// the callback persists the installation it 302s the browser here.
type BeginManagedSlackInstallRequest struct {
	RedirectURL string `json:"redirect_url"`
}

// BeginManagedSlackInstallResponse returns the Slack authorize URL the
// frontend sends the installer to, plus the state and its expiry for display.
type BeginManagedSlackInstallResponse struct {
	AuthorizeURL string `json:"authorize_url"`
	State        string `json:"state"`
	ExpiresAt    string `json:"expires_at"`
}

// managedSlackCallbackURL rebuilds the exact redirect_uri the authorize URL
// carried, so the token exchange presents the same value Slack authorized.
// Empty when PATCHBAY_PUBLIC_URL is unset — OAuth cannot work without an exact
// registered callback, so callers fail loudly instead of exchanging against a
// wrong URI.
func (h *Handler) managedSlackCallbackURL() string {
	base := strings.TrimRight(h.cfg.PublicURL, "/")
	if base == "" {
		return ""
	}
	return base + ManagedSlackOAuthCallbackPath
}

// BeginManagedSlackInstall (POST /api/workspaces/{id}/slack/install/managed)
// starts one hosted-OAuth authorization for the official Patchbay Slack app.
// Admin-gated at the router like the BYO install: both connect a
// workspace-level bot. The installer comes from the session, the workspace
// from the URL — the same boundary shape as RegisterSlackBYO.
//
// State issuance needs no client credentials, so it runs first; the authorize
// URL does, so a deployment without PATCHBAY_SLACK_CLIENT_ID/_SECRET (or
// without a public URL to build the callback from) mints the state and then
// fails loudly with 503 instead of handing out a URL that could never work.
func (h *Handler) BeginManagedSlackInstall(w http.ResponseWriter, r *http.Request) {
	if h.ManagedSlack == nil {
		writeError(w, http.StatusServiceUnavailable, "slack managed OAuth is not configured")
		return
	}
	userID, ok := requireUserID(w, r)
	if !ok {
		return
	}
	wsUUID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	installerUUID, ok := parseUUIDOrBadRequest(w, userID, "user id")
	if !ok {
		return
	}
	var body BeginManagedSlackInstallRequest
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	redirectURL := strings.TrimSpace(body.RedirectURL)
	if redirectURL == "" {
		writeError(w, http.StatusBadRequest, "redirect_url is required")
		return
	}
	state, expiresAt, err := h.ManagedSlack.BeginInstall(r.Context(), wsUUID, installerUUID, redirectURL)
	if err != nil {
		if errors.Is(err, slack.ErrInvalidRedirectURL) {
			writeError(w, http.StatusBadRequest, "redirect_url is not a valid absolute URL")
			return
		}
		writeError(w, http.StatusInternalServerError, "failed to start Slack authorization")
		return
	}
	clientID := h.ManagedSlack.ClientID()
	callbackURL := h.managedSlackCallbackURL()
	if clientID == "" || callbackURL == "" {
		writeError(w, http.StatusServiceUnavailable, "slack managed OAuth is not configured (missing client credentials or public URL)")
		return
	}
	writeJSON(w, http.StatusOK, BeginManagedSlackInstallResponse{
		AuthorizeURL: slack.AuthorizeURL(clientID, callbackURL, state),
		State:        state,
		ExpiresAt:    expiresAt.UTC().Format(time.RFC3339),
	})
}

// ManagedSlackOAuthCallback (GET /api/integrations/slack/oauth/callback) is
// the public landing for Slack's browser redirect. Query contract:
//   - error non-empty, or code/state empty: the installer denied (or Slack
//     never authorized) — 400, nothing is consumed.
//   - ConsumeState failure: unknown, expired, or already-consumed state all
//     render the same 400 restart-the-install answer, never distinguished.
//   - ExchangeCode failure: Slack refused the code — 502.
//   - Persist failure: 409 for a team owned elsewhere (or a second team in the
//     same workspace), 500 otherwise.
//   - Success: persist the team-keyed installation, broadcast
//     slack_installation:created like the BYO path, and 302 to the redirect_url
//     bound to the state.
func (h *Handler) ManagedSlackOAuthCallback(w http.ResponseWriter, r *http.Request) {
	if h.ManagedSlack == nil || h.SlackInstall == nil {
		writeError(w, http.StatusServiceUnavailable, "slack managed OAuth is not configured")
		return
	}
	query := r.URL.Query()
	if strings.TrimSpace(query.Get("error")) != "" {
		writeError(w, http.StatusBadRequest, "slack authorization was denied")
		return
	}
	code := strings.TrimSpace(query.Get("code"))
	state := strings.TrimSpace(query.Get("state"))
	if code == "" || state == "" {
		writeError(w, http.StatusBadRequest, "code and state are required")
		return
	}
	// Fail loudly before consuming the single-use state: with no client
	// credentials (or no public callback URL) the exchange could never succeed,
	// and consuming first would burn an authorization the operator could have
	// completed after fixing the config.
	if h.ManagedSlack.ClientID() == "" || h.managedSlackCallbackURL() == "" {
		writeError(w, http.StatusServiceUnavailable, "slack managed OAuth is not configured (missing client credentials or public URL)")
		return
	}
	claimed, err := h.ManagedSlack.ConsumeState(r.Context(), state)
	if err != nil {
		if errors.Is(err, slack.ErrInvalidOAuthState) {
			writeError(w, http.StatusBadRequest, "slack authorization expired or was already used — restart the install")
			return
		}
		writeError(w, http.StatusInternalServerError, "failed to finish Slack authorization")
		return
	}
	access, err := h.ManagedSlack.ExchangeCode(r.Context(), code, h.managedSlackCallbackURL())
	if err != nil {
		writeError(w, http.StatusBadGateway, "slack rejected the OAuth exchange")
		return
	}
	row, err := h.SlackInstall.RegisterManaged(r.Context(), slack.RegisterManagedParams{
		WorkspaceID: claimed.WorkspaceID,
		InstallerID: claimed.InstallerUserID,
		Access:      access,
	})
	if err != nil {
		switch {
		case errors.Is(err, slack.ErrTeamOwnedByAnotherWorkspace):
			writeError(w, http.StatusConflict, "this Slack workspace is already connected to a different Patchbay workspace — disconnect it there before connecting it here")
		case errors.Is(err, slack.ErrManagedAlreadyConnected):
			writeError(w, http.StatusConflict, err.Error())
		default:
			writeError(w, http.StatusInternalServerError, "failed to save Slack installation")
		}
		return
	}
	// Same broadcast as the BYO path so every open client invalidates its
	// installations query — the installer's tab is mid-redirect and will not
	// see the new bot otherwise.
	h.publishSlackInstallationCreated(row, uuidToString(claimed.InstallerUserID))
	http.Redirect(w, r, claimed.RedirectUrl, http.StatusFound)
}
