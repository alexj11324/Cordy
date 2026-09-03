package handler

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/patchbay-ai/patchbay/server/internal/events"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/slack"
	"github.com/patchbay-ai/patchbay/server/internal/testutil"
	"github.com/patchbay-ai/patchbay/server/internal/util/secretbox"
)

const managedTestPublicURL = "https://api.example.test"

// managedTestBox is a fixed at-rest key so the test can prove the persisted
// bot token is sealed, never plaintext.
func managedTestBox(t *testing.T) *secretbox.Box {
	t.Helper()
	key := make([]byte, secretbox.KeySize)
	for i := range key {
		key[i] = byte(0xA0 + i)
	}
	box, err := secretbox.New(key)
	if err != nil {
		t.Fatalf("secretbox.New: %v", err)
	}
	return box
}

// managedTokenServer fakes Slack's oauth.v2.access: the code selects which
// tenant the exchange returns, and an unknown code is refused like Slack's
// invalid_code. The redirect_uri is accepted blind — the handler under test is
// what guarantees begin and callback present the same value.
func managedTokenServer(t *testing.T) *httptest.Server {
	t.Helper()
	tenants := map[string]map[string]string{
		"code-happy": {"app_id": "AHAPPY", "team_id": "THAPPY", "bot_user_id": "UBHAPPY", "token": "xoxb-happy"},
		"code-ta":    {"app_id": "ATEAMS", "team_id": "TAFIRST", "bot_user_id": "UBTA", "token": "xoxb-ta"},
		"code-tb":    {"app_id": "ATEAMS", "team_id": "TBSECOND", "bot_user_id": "UBTB", "token": "xoxb-tb"},
	}
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		tenant, ok := tenants[r.FormValue("code")]
		if !ok {
			_ = json.NewEncoder(w).Encode(map[string]any{"ok": false, "error": "invalid_code"})
			return
		}
		_ = json.NewEncoder(w).Encode(map[string]any{
			"ok":           true,
			"access_token": tenant["token"],
			"app_id":       tenant["app_id"],
			"bot_user_id":  tenant["bot_user_id"],
			"team":         map[string]string{"id": tenant["team_id"]},
			"authed_user":  map[string]string{"id": "UINSTALLER"},
		})
	}))
}

// newManagedSlackTestHandler builds a handler with real OAuth + install
// services against the shared test database. It is deliberately NOT the shared
// testHandler: wiring ManagedSlack onto the shared handler would leak into
// unrelated tests. Callers clean up their slack rows via
// cleanupManagedSlackRows.
func newManagedSlackTestHandler(t *testing.T, clientID string, tokenServer *httptest.Server) *Handler {
	t.Helper()
	oauthSvc, err := slack.NewManagedOAuthService(slack.ManagedOAuthConfig{
		Queries:      testHandler.Queries,
		ClientID:     clientID,
		ClientSecret: "csec-test",
		HTTPClient:   tokenServer.Client(),
		TokenURL:     tokenServer.URL,
	})
	if err != nil {
		t.Fatalf("NewManagedOAuthService: %v", err)
	}
	installSvc, err := slack.NewInstallService(testHandler.Queries, testPool, managedTestBox(t), nil)
	if err != nil {
		t.Fatalf("NewInstallService: %v", err)
	}
	return &Handler{
		Queries:      testHandler.Queries,
		Bus:          events.New(),
		ManagedSlack: oauthSvc,
		SlackInstall: installSvc,
		cfg:          Config{PublicURL: managedTestPublicURL},
	}
}

func cleanupManagedSlackRows(t *testing.T) {
	t.Helper()
	t.Cleanup(func() {
		ctx := context.Background()
		_, _ = testPool.Exec(ctx, `DELETE FROM channel_installation WHERE workspace_id = $1 AND channel_type = 'slack'`, testWorkspaceID)
		_, _ = testPool.Exec(ctx, `DELETE FROM slack_oauth_state WHERE workspace_id = $1`, testWorkspaceID)
	})
}

func beginManagedInstallRequest(t *testing.T, h *Handler, redirectURL string) *testutil.Response {
	t.Helper()
	req := newRequest("POST", "/api/workspaces/"+testWorkspaceID+"/slack/install/managed", map[string]any{
		"redirect_url": redirectURL,
	})
	req = withURLParam(req, "id", testWorkspaceID)
	return testutil.Call(t, h.BeginManagedSlackInstall, req)
}

func TestBeginManagedSlackInstallServiceDisabled(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	req := withURLParam(newRequest("POST", "/", map[string]any{"redirect_url": "https://app.example.test/done"}), "id", testWorkspaceID)
	testutil.Call(t, (&Handler{}).BeginManagedSlackInstall, req).Want(http.StatusServiceUnavailable)
}

func TestBeginManagedSlackInstallRequiresRedirectURL(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	srv := managedTokenServer(t)
	defer srv.Close()
	h := newManagedSlackTestHandler(t, "cid-test", srv)

	beginManagedInstallRequest(t, h, "").Want(http.StatusBadRequest)
	beginManagedInstallRequest(t, h, "not-a-url").Want(http.StatusBadRequest)
}

// Without client credentials the begin still mints state (so the install can
// complete once the operator configures the hosted app) but refuses the
// authorize URL with 503 — fail loudly instead of handing out a dead link.
func TestBeginManagedSlackInstallUnconfiguredClientFailsLoudly(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	cleanupManagedSlackRows(t)
	srv := managedTokenServer(t)
	defer srv.Close()
	h := newManagedSlackTestHandler(t, "", srv)

	w := beginManagedInstallRequest(t, h, "https://app.example.test/done").Want(http.StatusServiceUnavailable)
	if !strings.Contains(w.Body.String(), "not configured") {
		t.Fatalf("503 body should name the missing configuration, got %s", w.Body.String())
	}
	var states int
	dbfx.QueryRow(t, `SELECT COUNT(*) FROM slack_oauth_state WHERE workspace_id = $1`, testWorkspaceID).Scan(&states)
	if states != 1 {
		t.Fatalf("unconfigured begin must still mint state (got %d rows), so the install survives operator setup", states)
	}
}

func TestBeginManagedSlackInstallHappyPath(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	cleanupManagedSlackRows(t)
	srv := managedTokenServer(t)
	defer srv.Close()
	h := newManagedSlackTestHandler(t, "cid-happy", srv)

	var begun BeginManagedSlackInstallResponse
	beginManagedInstallRequest(t, h, "https://app.example.test/done").Want(http.StatusOK).JSON(&begun)
	if begun.State == "" || begun.ExpiresAt == "" {
		t.Fatalf("begin must return state + expiry, got %+v", begun)
	}
	if !strings.Contains(begun.AuthorizeURL, "client_id=cid-happy") || !strings.Contains(begun.AuthorizeURL, "state="+begun.State) {
		t.Fatalf("authorize_url must carry the client id and minted state, got %q", begun.AuthorizeURL)
	}
	wantRedirect := managedTestPublicURL + ManagedSlackOAuthCallbackPath
	if !strings.Contains(begun.AuthorizeURL, "redirect_uri=") || !strings.Contains(begun.AuthorizeURL, "api.example.test") {
		t.Fatalf("authorize_url must point Slack back at %s, got %q", wantRedirect, begun.AuthorizeURL)
	}
	// The minted state round-trips through the store (and consuming it here
	// leaves no row behind for later tests).
	if _, err := h.ManagedSlack.ConsumeState(context.Background(), begun.State); err != nil {
		t.Fatalf("minted state must be consumable: %v", err)
	}
}

func TestManagedSlackOAuthCallbackQueryValidation(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	srv := managedTokenServer(t)
	defer srv.Close()
	h := newManagedSlackTestHandler(t, "cid-test", srv)

	for name, query := range map[string]string{
		"denied authorization": "error=access_denied&state=whatever",
		"missing code":         "state=whatever",
		"missing state":        "code=whatever",
		"unknown state":        "code=whatever&state=definitely-not-minted",
	} {
		t.Run(name, func(t *testing.T) {
			req := httptest.NewRequest(http.MethodGet, ManagedSlackOAuthCallbackPath+"?"+query, nil)
			testutil.Call(t, h.ManagedSlackOAuthCallback, req).Want(http.StatusBadRequest)
		})
	}
}

func TestManagedSlackOAuthCallbackServiceDisabled(t *testing.T) {
	req := httptest.NewRequest(http.MethodGet, ManagedSlackOAuthCallbackPath+"?code=x&state=y", nil)
	testutil.Call(t, (&Handler{}).ManagedSlackOAuthCallback, req).Want(http.StatusServiceUnavailable)
}

// Full loop with a faked Slack token endpoint: mint state through the service,
// land the callback, and expect a 302 to the state-bound redirect plus a
// team-keyed, sealed installation row.
func TestManagedSlackOAuthCallbackHappyPathRedirects(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	cleanupManagedSlackRows(t)
	srv := managedTokenServer(t)
	defer srv.Close()
	h := newManagedSlackTestHandler(t, "cid-happy", srv)

	const redirectURL = "https://app.example.test/done"
	state, _, err := h.ManagedSlack.BeginInstall(context.Background(), parseUUID(testWorkspaceID), parseUUID(testUserID), redirectURL)
	if err != nil {
		t.Fatalf("begin: %v", err)
	}
	req := httptest.NewRequest(http.MethodGet, ManagedSlackOAuthCallbackPath+"?code=code-happy&state="+state, nil)
	w := testutil.Call(t, h.ManagedSlackOAuthCallback, req).Want(http.StatusFound)
	if got := w.Header().Get("Location"); got != redirectURL {
		t.Fatalf("callback must 302 to the state-bound redirect %q, got %q", redirectURL, got)
	}

	var configJSON []byte
	var agentID string
	var status string
	dbfx.QueryRow(t, `SELECT config, agent_id::text, status FROM channel_installation WHERE workspace_id = $1 AND channel_type = 'slack'`,
		testWorkspaceID).Scan(&configJSON, &agentID, &status)
	if status != "installed" {
		t.Fatalf("installation status = %q, want installed", status)
	}
	if agentID != "00000000-0000-0000-0000-000000000000" {
		t.Fatalf("managed install agent = %s, want the nil UUID (workspace-owned, team-keyed)", agentID)
	}
	var cfg struct {
		AppID             string `json:"app_id"`
		ApiAppID          string `json:"api_app_id"`
		TeamID            string `json:"team_id"`
		BotUserID         string `json:"bot_user_id"`
		BotTokenEncrypted string `json:"bot_token_encrypted"`
		AppTokenEncrypted string `json:"app_token_encrypted"`
		Transport         string `json:"transport"`
	}
	if err := json.Unmarshal(configJSON, &cfg); err != nil {
		t.Fatalf("stored config is not JSON: %v", err)
	}
	if cfg.AppID != "AHAPPY:THAPPY" || cfg.ApiAppID != "AHAPPY" || cfg.TeamID != "THAPPY" || cfg.BotUserID != "UBHAPPY" {
		t.Fatalf("stored routing identity = %+v, want the team-keyed composite", cfg)
	}
	if cfg.Transport != "webhook" || cfg.AppTokenEncrypted != "" {
		t.Fatalf("managed install must be webhook transport with no app token, got %+v", cfg)
	}
	raw, err := base64.StdEncoding.DecodeString(cfg.BotTokenEncrypted)
	if err != nil {
		t.Fatalf("bot token is not base64 ciphertext: %v", err)
	}
	plain, err := managedTestBox(t).Open(raw)
	if err != nil {
		t.Fatalf("stored bot token does not open with the deployment key: %v", err)
	}
	if string(plain) != "xoxb-happy" {
		t.Fatalf("unsealed bot token = %q, want the exchanged token", plain)
	}
}

// One managed install per workspace: connecting a second Slack team while the
// first is live is a 409, not a silent steal or a duplicate row.
func TestManagedSlackOAuthCallbackSecondTeamConflicts(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	cleanupManagedSlackRows(t)
	srv := managedTokenServer(t)
	defer srv.Close()
	h := newManagedSlackTestHandler(t, "cid-teams", srv)

	wsUUID, userUUID := parseUUID(testWorkspaceID), parseUUID(testUserID)
	stateA, _, err := h.ManagedSlack.BeginInstall(context.Background(), wsUUID, userUUID, "https://app.example.test/a")
	if err != nil {
		t.Fatalf("begin A: %v", err)
	}
	req := httptest.NewRequest(http.MethodGet, ManagedSlackOAuthCallbackPath+"?code=code-ta&state="+stateA, nil)
	testutil.Call(t, h.ManagedSlackOAuthCallback, req).Want(http.StatusFound)

	stateB, _, err := h.ManagedSlack.BeginInstall(context.Background(), wsUUID, userUUID, "https://app.example.test/b")
	if err != nil {
		t.Fatalf("begin B: %v", err)
	}
	req = httptest.NewRequest(http.MethodGet, ManagedSlackOAuthCallbackPath+"?code=code-tb&state="+stateB, nil)
	testutil.Call(t, h.ManagedSlackOAuthCallback, req).Want(http.StatusConflict)

	var installs int
	dbfx.QueryRow(t, `SELECT COUNT(*) FROM channel_installation WHERE workspace_id = $1 AND channel_type = 'slack' AND status = 'installed'`,
		testWorkspaceID).Scan(&installs)
	if installs != 1 {
		t.Fatalf("second team must not duplicate the install (installed records = %d)", installs)
	}
}
