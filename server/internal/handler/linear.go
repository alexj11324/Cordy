package handler

import (
	"context"
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/featureflags"
	"github.com/patchbay-ai/patchbay/server/internal/middleware"
)

const (
	linearAuthorizeURL = "https://linear.app/oauth/authorize"
	linearTokenURL     = "https://api.linear.app/oauth/token"
	linearGraphQLURL   = "https://api.linear.app/graphql"
)

type linearConnection struct {
	ID, WorkspaceID, OrganizationID, OrganizationName, ActorID string
	Scopes                                                     []string
	WebhookID                                                  *string
	Status, TokenExpiresAt                                     string
	LastSuccessAt, LastError                                   *string
	CreatedAt, UpdatedAt                                       string
}

func (c linearConnection) response() map[string]any {
	return map[string]any{"id": c.ID, "workspace_id": c.WorkspaceID, "organization_id": c.OrganizationID,
		"organization_name": c.OrganizationName, "actor_id": c.ActorID, "scopes": c.Scopes,
		"webhook_id": c.WebhookID, "status": c.Status, "token_expires_at": c.TokenExpiresAt,
		"last_success_at": c.LastSuccessAt, "last_error": c.LastError, "created_at": c.CreatedAt, "updated_at": c.UpdatedAt}
}

func (h *Handler) linearEnabled(ctx context.Context) bool {
	return featureflags.LinearInstallationFoundationEnabled(ctx, h.FeatureFlags)
}

func (h *Handler) linearConfigured() bool {
	return h.LinearSecretBox != nil && h.LinearClientID != "" && h.LinearClientSecret != "" && h.LinearWebhookSecret != ""
}

func (h *Handler) requireLinear(w http.ResponseWriter, r *http.Request) bool {
	if !h.linearEnabled(r.Context()) {
		writeError(w, http.StatusNotFound, "Linear integration is not enabled")
		return false
	}
	return true
}

func scanLinearConnection(row pgx.Row) (*linearConnection, error) {
	var c linearConnection
	var id, ws pgtype.UUID
	var scopes []byte
	var webhook, lastSuccess, lastError pgtype.Text
	var expires, created, updated pgtype.Timestamptz
	err := row.Scan(&id, &ws, &c.OrganizationID, &c.OrganizationName, &c.ActorID, &scopes, &webhook,
		&c.Status, &expires, &lastSuccess, &lastError, &created, &updated)
	if err != nil {
		return nil, err
	}
	c.ID, c.WorkspaceID = uuidToString(id), uuidToString(ws)
	_ = json.Unmarshal(scopes, &c.Scopes)
	if c.Scopes == nil {
		c.Scopes = []string{}
	}
	if webhook.Valid {
		c.WebhookID = &webhook.String
	}
	if lastSuccess.Valid {
		c.LastSuccessAt = &lastSuccess.String
	}
	if lastError.Valid {
		c.LastError = &lastError.String
	}
	c.TokenExpiresAt, c.CreatedAt, c.UpdatedAt = timestampToString(expires), timestampToString(created), timestampToString(updated)
	return &c, nil
}

const linearConnectionSelect = `SELECT id, workspace_id, organization_id, organization_name, actor_id, scopes,
webhook_id, status, token_expires_at, last_success_at::text, last_error, created_at, updated_at
FROM linear_connection WHERE workspace_id=$1 AND status <> 'revoked' ORDER BY created_at DESC LIMIT 1`

func (h *Handler) GetLinearConnection(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) {
		return
	}
	ws, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	c, err := scanLinearConnection(h.DB.QueryRow(r.Context(), linearConnectionSelect, ws))
	if errors.Is(err, pgx.ErrNoRows) {
		writeJSON(w, http.StatusOK, map[string]any{"configured": h.linearConfigured(), "connected": false,
			"pull_import_enabled": h.LinearPullEnabled, "push_enabled": h.LinearPushEnabled, "connection": nil})
		return
	}
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to load Linear connection")
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"configured": h.linearConfigured(), "connected": true,
		"pull_import_enabled": h.LinearPullEnabled, "push_enabled": h.LinearPushEnabled, "connection": c.response()})
}

func randomURLToken(n int) (string, error) {
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		return "", err
	}
	return base64.RawURLEncoding.EncodeToString(b), nil
}

func (h *Handler) ConnectLinear(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) {
		return
	}
	if !h.linearConfigured() || strings.TrimSpace(h.cfg.PublicURL) == "" {
		writeError(w, http.StatusServiceUnavailable, "Linear OAuth is not configured")
		return
	}
	ws, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	member, ok := middleware.MemberFromContext(r.Context())
	if !ok {
		writeError(w, http.StatusUnauthorized, "member context missing")
		return
	}
	state, err := randomURLToken(32)
	if err != nil {
		writeError(w, 500, "failed to create OAuth state")
		return
	}
	verifier, err := randomURLToken(48)
	if err != nil {
		writeError(w, 500, "failed to create PKCE verifier")
		return
	}
	sealed, err := h.LinearSecretBox.Seal([]byte(verifier))
	if err != nil {
		writeError(w, 500, "failed to protect OAuth state")
		return
	}
	stateSum, verifierSum := sha256.Sum256([]byte(state)), sha256.Sum256([]byte(verifier))
	callback := strings.TrimRight(h.cfg.PublicURL, "/") + "/api/linear/oauth/callback"
	expires := time.Now().UTC().Add(10 * time.Minute)
	_, err = h.DB.Exec(r.Context(), `INSERT INTO linear_oauth_state
(id,state_hash,workspace_id,user_id,code_verifier_encrypted,redirect_uri,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7)`,
		parseUUID(uuid.NewString()), hex.EncodeToString(stateSum[:]), ws, member.UserID, sealed, callback, expires)
	if err != nil {
		writeError(w, 500, "failed to store OAuth state")
		return
	}
	q := url.Values{"client_id": {h.LinearClientID}, "redirect_uri": {callback}, "response_type": {"code"},
		"scope": {"read,write"}, "state": {state}, "code_challenge": {base64.RawURLEncoding.EncodeToString(verifierSum[:])},
		"code_challenge_method": {"S256"}, "actor": {"app"}}
	writeJSON(w, http.StatusOK, map[string]any{"authorization_url": linearAuthorizeURL + "?" + q.Encode(), "state_expires_at": expires.Format(time.RFC3339Nano)})
}

type linearTokenResponse struct {
	AccessToken, RefreshToken, Scope, TokenType string
	ExpiresIn                                   int64
}

func (t *linearTokenResponse) UnmarshalJSON(b []byte) error {
	var raw struct {
		AccessToken  string `json:"access_token"`
		RefreshToken string `json:"refresh_token"`
		Scope        string `json:"scope"`
		TokenType    string `json:"token_type"`
		ExpiresIn    int64  `json:"expires_in"`
	}
	if err := json.Unmarshal(b, &raw); err != nil {
		return err
	}
	t.AccessToken, t.RefreshToken, t.Scope, t.TokenType, t.ExpiresIn = raw.AccessToken, raw.RefreshToken, raw.Scope, raw.TokenType, raw.ExpiresIn
	return nil
}

func (h *Handler) LinearOAuthCallback(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) || !h.linearConfigured() {
		return
	}
	code, state := strings.TrimSpace(r.URL.Query().Get("code")), strings.TrimSpace(r.URL.Query().Get("state"))
	if code == "" || state == "" {
		writeError(w, 400, "code and state are required")
		return
	}
	sum := sha256.Sum256([]byte(state))
	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		writeError(w, 500, "failed to begin OAuth callback")
		return
	}
	defer tx.Rollback(r.Context())
	var ws, user pgtype.UUID
	var sealed []byte
	var redirect string
	err = tx.QueryRow(r.Context(), `UPDATE linear_oauth_state SET consumed_at=now() WHERE state_hash=$1 AND consumed_at IS NULL AND expires_at>now()
RETURNING workspace_id,user_id,code_verifier_encrypted,redirect_uri`, hex.EncodeToString(sum[:])).Scan(&ws, &user, &sealed, &redirect)
	if errors.Is(err, pgx.ErrNoRows) {
		writeError(w, 400, "OAuth state is invalid, expired, or already used")
		return
	}
	if err != nil {
		writeError(w, 500, "failed to consume OAuth state")
		return
	}
	verifier, err := h.LinearSecretBox.Open(sealed)
	if err != nil {
		writeError(w, 500, "failed to open OAuth state")
		return
	}
	form := url.Values{"grant_type": {"authorization_code"}, "code": {code}, "redirect_uri": {redirect}, "client_id": {h.LinearClientID}, "client_secret": {h.LinearClientSecret}, "code_verifier": {string(verifier)}}
	req, _ := http.NewRequestWithContext(r.Context(), http.MethodPost, linearTokenURL, strings.NewReader(form.Encode()))
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		writeError(w, 502, "Linear token exchange failed")
		return
	}
	defer resp.Body.Close()
	if resp.StatusCode/100 != 2 {
		writeError(w, 502, "Linear rejected the OAuth exchange")
		return
	}
	var token linearTokenResponse
	if err := json.NewDecoder(io.LimitReader(resp.Body, 1<<20)).Decode(&token); err != nil || token.AccessToken == "" {
		writeError(w, 502, "Linear returned an invalid token response")
		return
	}
	actorID, orgID, orgName, err := linearIdentity(r.Context(), token.AccessToken)
	if err != nil {
		writeError(w, 502, "failed to load Linear organization")
		return
	}
	access, err := h.LinearSecretBox.Seal([]byte(token.AccessToken))
	if err != nil {
		writeError(w, 500, "failed to encrypt Linear token")
		return
	}
	refresh, err := h.LinearSecretBox.Seal([]byte(token.RefreshToken))
	if err != nil {
		writeError(w, 500, "failed to encrypt Linear token")
		return
	}
	scopes, _ := json.Marshal(strings.FieldsFunc(token.Scope, func(r rune) bool { return r == ',' || r == ' ' }))
	if token.ExpiresIn <= 0 {
		token.ExpiresIn = 30 * 24 * 60 * 60
	}
	_, err = tx.Exec(r.Context(), `INSERT INTO linear_connection(id,workspace_id,organization_id,organization_name,actor_id,access_token_encrypted,refresh_token_encrypted,token_expires_at,scopes,created_by_id)
VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT (workspace_id) DO UPDATE SET organization_id=EXCLUDED.organization_id,organization_name=EXCLUDED.organization_name,
actor_id=EXCLUDED.actor_id,access_token_encrypted=EXCLUDED.access_token_encrypted,refresh_token_encrypted=EXCLUDED.refresh_token_encrypted,token_expires_at=EXCLUDED.token_expires_at,scopes=EXCLUDED.scopes,status='active',last_error=NULL,updated_at=now()`,
		parseUUID(uuid.NewString()), ws, orgID, orgName, actorID, access, refresh, time.Now().UTC().Add(time.Duration(token.ExpiresIn)*time.Second), scopes, user)
	if err != nil {
		writeError(w, 500, "failed to save Linear connection")
		return
	}
	if err := tx.Commit(r.Context()); err != nil {
		writeError(w, 500, "failed to commit Linear connection")
		return
	}
	var slug string
	_ = h.DB.QueryRow(r.Context(), `SELECT slug FROM workspace WHERE id=$1`, ws).Scan(&slug)
	target := strings.TrimRight(h.cfg.AppURL, "/")
	if slug != "" {
		target += "/" + url.PathEscape(slug) + "/settings?tab=integrations&linear_connected=1"
	}
	if target == "" {
		writeJSON(w, 200, map[string]bool{"connected": true})
		return
	}
	http.Redirect(w, r, target, http.StatusFound)
}

func linearGraphQL(ctx context.Context, token, query string, variables map[string]any, out any) error {
	body, _ := json.Marshal(map[string]any{"query": query, "variables": variables})
	req, _ := http.NewRequestWithContext(ctx, http.MethodPost, linearGraphQLURL, strings.NewReader(string(body)))
	req.Header.Set("Authorization", token)
	req.Header.Set("Content-Type", "application/json")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode/100 != 2 {
		return fmt.Errorf("linear graphql status %d", resp.StatusCode)
	}
	var envelope struct {
		Data   json.RawMessage `json:"data"`
		Errors []struct {
			Message string `json:"message"`
		} `json:"errors"`
	}
	if err := json.NewDecoder(io.LimitReader(resp.Body, 4<<20)).Decode(&envelope); err != nil {
		return err
	}
	if len(envelope.Errors) > 0 {
		return errors.New(envelope.Errors[0].Message)
	}
	return json.Unmarshal(envelope.Data, out)
}

func linearIdentity(ctx context.Context, token string) (string, string, string, error) {
	var data struct {
		Viewer struct {
			ID string `json:"id"`
		} `json:"viewer"`
		Organization *struct {
			ID   string `json:"id"`
			Name string `json:"name"`
		} `json:"organization"`
	}
	err := linearGraphQL(ctx, token, `query PatchbayLinearIdentity { viewer { id } organization { id name } }`, nil, &data)
	if err != nil {
		return "", "", "", fmt.Errorf("identity: %w", err)
	}
	if data.Organization == nil {
		return "", "", "", errors.New("identity: Linear organization is missing")
	}
	return data.Viewer.ID, data.Organization.ID, data.Organization.Name, nil
}

func (h *Handler) linearAccessToken(ctx context.Context, ws pgtype.UUID) (string, error) {
	var sealed []byte
	var expires time.Time
	err := h.DB.QueryRow(ctx, `SELECT access_token_encrypted,token_expires_at FROM linear_connection WHERE workspace_id=$1 AND status='active'`, ws).Scan(&sealed, &expires)
	if err != nil {
		return "", err
	}
	if time.Now().After(expires) {
		return "", errors.New("Linear authorization expired")
	}
	plain, err := h.LinearSecretBox.Open(sealed)
	return string(plain), err
}

func (h *Handler) GetLinearCatalog(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) {
		return
	}
	ws, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	if !h.linearConfigured() {
		writeError(w, 503, "Linear integration is not configured")
		return
	}
	token, err := h.linearAccessToken(r.Context(), ws)
	if err != nil {
		writeError(w, 409, "Linear connection requires authorization")
		return
	}
	var data struct {
		Teams struct {
			Nodes []map[string]any `json:"nodes"`
		} `json:"teams"`
		Projects struct {
			Nodes []map[string]any `json:"nodes"`
		} `json:"projects"`
		WorkflowStates struct {
			Nodes []map[string]any `json:"nodes"`
		} `json:"workflowStates"`
		Users struct {
			Nodes []map[string]any `json:"nodes"`
		} `json:"users"`
		IssueLabels struct {
			Nodes []map[string]any `json:"nodes"`
		} `json:"issueLabels"`
	}
	query := `query PatchbayLinearCatalog { teams(first:250){nodes{id key name}} projects(first:250){nodes{id name}} workflowStates(first:250){nodes{id name type color}} users(first:250){nodes{id name email}} issueLabels(first:250){nodes{id name color isGroup parent{id} team{id}}} }`
	if err := linearGraphQL(r.Context(), token, query, nil, &data); err != nil {
		writeError(w, 502, "failed to load Linear catalog")
		return
	}
	labels := make([]map[string]any, 0, len(data.IssueLabels.Nodes))
	for _, label := range data.IssueLabels.Nodes {
		if group, ok := label["isGroup"]; ok {
			label["is_group"] = group
			delete(label, "isGroup")
		}
		if parent, ok := label["parent"].(map[string]any); ok {
			label["parent_id"] = parent["id"]
		} else {
			label["parent_id"] = nil
		}
		delete(label, "parent")
		if team, ok := label["team"].(map[string]any); ok {
			label["team_id"] = team["id"]
		} else {
			label["team_id"] = nil
		}
		delete(label, "team")
		labels = append(labels, label)
	}
	writeJSON(w, 200, map[string]any{"teams": data.Teams.Nodes, "projects": data.Projects.Nodes, "states": data.WorkflowStates.Nodes, "users": data.Users.Nodes, "labels": labels})
}

type linearBindingRequest struct {
	ConnectionID, PatchbayProjectID, LinearProjectID string
	LinearTeamID                                     *string
	Status, SyncMode                                 string
	InitialSourceOfTruth                             *string
	StatusMapping, AgentLabelMapping                 map[string]any
}

func (v *linearBindingRequest) UnmarshalJSON(b []byte) error {
	var r struct {
		ConnectionID      string         `json:"connection_id"`
		PatchbayProjectID string         `json:"patchbay_project_id"`
		LinearProjectID   string         `json:"linear_project_id"`
		LinearTeamID      *string        `json:"linear_team_id"`
		Status            string         `json:"status"`
		SyncMode          string         `json:"sync_mode"`
		InitialSource     *string        `json:"initial_source_of_truth"`
		StatusMapping     map[string]any `json:"status_mapping"`
		AgentMapping      map[string]any `json:"agent_label_mapping"`
	}
	if err := json.Unmarshal(b, &r); err != nil {
		return err
	}
	v.ConnectionID, v.PatchbayProjectID, v.LinearProjectID, v.LinearTeamID, v.Status, v.SyncMode, v.InitialSourceOfTruth, v.StatusMapping, v.AgentLabelMapping = r.ConnectionID, r.PatchbayProjectID, r.LinearProjectID, r.LinearTeamID, r.Status, r.SyncMode, r.InitialSource, r.StatusMapping, r.AgentMapping
	return nil
}

const linearBindingSelect = `SELECT id,workspace_id,connection_id,patchbay_project_id,linear_project_id,linear_team_id,status,sync_mode,initial_source_of_truth,status_mapping,agent_label_mapping,activated_at,paused_at,created_by_id,created_at,updated_at FROM linear_project_binding`

func scanLinearBinding(row pgx.Row) (map[string]any, error) {
	var id, ws, cid, pid, creator pgtype.UUID
	var remote, status, mode string
	var team, source pgtype.Text
	var sm, am []byte
	var activated, paused pgtype.Timestamptz
	var created, updated time.Time
	if err := row.Scan(&id, &ws, &cid, &pid, &remote, &team, &status, &mode, &source, &sm, &am, &activated, &paused, &creator, &created, &updated); err != nil {
		return nil, err
	}
	var smv, amv map[string]any
	_ = json.Unmarshal(sm, &smv)
	_ = json.Unmarshal(am, &amv)
	return map[string]any{"id": uuidToString(id), "workspace_id": uuidToString(ws), "connection_id": uuidToString(cid), "patchbay_project_id": uuidToString(pid), "linear_project_id": remote, "linear_team_id": textToPtr(team), "status": status, "sync_mode": mode, "initial_source_of_truth": textToPtr(source), "status_mapping": smv, "agent_label_mapping": amv, "activated_at": timestampToPtr(activated), "paused_at": timestampToPtr(paused), "created_by_id": uuidToString(creator), "created_at": created.Format(time.RFC3339Nano), "updated_at": updated.Format(time.RFC3339Nano)}, nil
}

func (h *Handler) ListLinearBindings(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) {
		return
	}
	ws, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	rows, err := h.DB.Query(r.Context(), linearBindingSelect+` WHERE workspace_id=$1 AND status<>'tombstone' ORDER BY created_at`, ws)
	if err != nil {
		writeError(w, 500, "failed to list Linear bindings")
		return
	}
	defer rows.Close()
	out := []map[string]any{}
	for rows.Next() {
		b, e := scanLinearBinding(rows)
		if e != nil {
			writeError(w, 500, "failed to read Linear bindings")
			return
		}
		out = append(out, b)
	}
	writeJSON(w, 200, map[string]any{"bindings": out})
}
func validLinearBinding(v linearBindingRequest) bool {
	switch v.SyncMode {
	case "import", "publish", "two_way", "not_synced":
	default:
		return false
	}
	if v.Status == "" {
		v.Status = "draft"
	}
	switch v.Status {
	case "draft", "active", "paused", "tombstone":
	default:
		return false
	}
	return v.ConnectionID != "" && v.PatchbayProjectID != "" && v.LinearProjectID != ""
}
func (h *Handler) CreateLinearBinding(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) {
		return
	}
	ws, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	var v linearBindingRequest
	if json.NewDecoder(r.Body).Decode(&v) != nil || !validLinearBinding(v) {
		writeError(w, 400, "invalid Linear binding")
		return
	}
	cid, ok := parseUUIDOrBadRequest(w, v.ConnectionID, "connection id")
	if !ok {
		return
	}
	pid, ok := parseUUIDOrBadRequest(w, v.PatchbayProjectID, "project id")
	if !ok {
		return
	}
	member, _ := middleware.MemberFromContext(r.Context())
	if v.Status == "" {
		v.Status = "draft"
	}
	sm, _ := json.Marshal(v.StatusMapping)
	am, _ := json.Marshal(v.AgentLabelMapping)
	id := parseUUID(uuid.NewString())
	_, err := h.DB.Exec(r.Context(), `INSERT INTO linear_project_binding(id,workspace_id,connection_id,patchbay_project_id,linear_project_id,linear_team_id,status,sync_mode,initial_source_of_truth,status_mapping,agent_label_mapping,activated_at,paused_at,created_by_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,CASE WHEN $7='active' THEN now() END,CASE WHEN $7='paused' THEN now() END,$12)`, id, ws, cid, pid, v.LinearProjectID, v.LinearTeamID, v.Status, v.SyncMode, v.InitialSourceOfTruth, sm, am, member.UserID)
	if err != nil {
		writeError(w, 409, "Linear binding conflicts with an existing mapping")
		return
	}
	b, err := scanLinearBinding(h.DB.QueryRow(r.Context(), linearBindingSelect+` WHERE id=$1`, id))
	if err != nil {
		writeError(w, 500, "failed to load Linear binding")
		return
	}
	writeJSON(w, 201, b)
}
func (h *Handler) UpdateLinearBinding(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) {
		return
	}
	ws, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	bid, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "bindingId"), "binding id")
	if !ok {
		return
	}
	var v linearBindingRequest
	if json.NewDecoder(r.Body).Decode(&v) != nil || !validLinearBinding(v) {
		writeError(w, 400, "invalid Linear binding")
		return
	}
	cid, ok := parseUUIDOrBadRequest(w, v.ConnectionID, "connection id")
	if !ok {
		return
	}
	pid, ok := parseUUIDOrBadRequest(w, v.PatchbayProjectID, "project id")
	if !ok {
		return
	}
	sm, _ := json.Marshal(v.StatusMapping)
	am, _ := json.Marshal(v.AgentLabelMapping)
	tag, err := h.DB.Exec(r.Context(), `UPDATE linear_project_binding SET connection_id=$3,patchbay_project_id=$4,linear_project_id=$5,linear_team_id=$6,status=$7,sync_mode=$8,initial_source_of_truth=$9,status_mapping=$10,agent_label_mapping=$11,activated_at=CASE WHEN $7='active' THEN COALESCE(activated_at,now()) END,paused_at=CASE WHEN $7='paused' THEN now() END,updated_at=now() WHERE id=$1 AND workspace_id=$2`, bid, ws, cid, pid, v.LinearProjectID, v.LinearTeamID, v.Status, v.SyncMode, v.InitialSourceOfTruth, sm, am)
	if err != nil {
		writeError(w, 409, "Linear binding conflicts with an existing mapping")
		return
	}
	if tag.RowsAffected() == 0 {
		writeError(w, 404, "Linear binding not found")
		return
	}
	b, _ := scanLinearBinding(h.DB.QueryRow(r.Context(), linearBindingSelect+` WHERE id=$1`, bid))
	writeJSON(w, 200, b)
}
func (h *Handler) DeleteLinearBinding(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) {
		return
	}
	ws, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	bid, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "bindingId"), "binding id")
	if !ok {
		return
	}
	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		writeError(w, 500, "failed to delete Linear binding")
		return
	}
	defer tx.Rollback(r.Context())
	for _, q := range []string{`DELETE FROM linear_sync_conflict WHERE binding_id=$1 AND workspace_id=$2`, `DELETE FROM linear_sync_outbox WHERE binding_id=$1 AND workspace_id=$2`, `DELETE FROM linear_issue_link WHERE binding_id=$1 AND workspace_id=$2`, `DELETE FROM linear_project_binding WHERE id=$1 AND workspace_id=$2`} {
		if _, err = tx.Exec(r.Context(), q, bid, ws); err != nil {
			writeError(w, 500, "failed to delete Linear binding")
			return
		}
	}
	if err = tx.Commit(r.Context()); err != nil {
		writeError(w, 500, "failed to delete Linear binding")
		return
	}
	w.WriteHeader(204)
}

func (h *Handler) ListLinearMemberBindings(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) {
		return
	}
	ws, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	rows, err := h.DB.Query(r.Context(), `SELECT id,workspace_id,connection_id,patchbay_user_id,linear_user_id,created_at,updated_at FROM linear_member_binding WHERE workspace_id=$1 ORDER BY created_at`, ws)
	if err != nil {
		writeError(w, 500, "failed to list Linear member bindings")
		return
	}
	defer rows.Close()
	out := []map[string]any{}
	for rows.Next() {
		var id, wid, cid, uid pgtype.UUID
		var linear string
		var created, updated time.Time
		if rows.Scan(&id, &wid, &cid, &uid, &linear, &created, &updated) != nil {
			writeError(w, 500, "failed to read Linear member bindings")
			return
		}
		out = append(out, map[string]any{"id": uuidToString(id), "workspace_id": uuidToString(wid), "connection_id": uuidToString(cid), "patchbay_user_id": uuidToString(uid), "linear_user_id": linear, "created_at": created.Format(time.RFC3339Nano), "updated_at": updated.Format(time.RFC3339Nano)})
	}
	writeJSON(w, 200, map[string]any{"bindings": out})
}
func (h *Handler) UpsertLinearMemberBinding(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) {
		return
	}
	ws, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	var v struct {
		ConnectionID   string `json:"connection_id"`
		PatchbayUserID string `json:"patchbay_user_id"`
		LinearUserID   string `json:"linear_user_id"`
	}
	if json.NewDecoder(r.Body).Decode(&v) != nil || v.LinearUserID == "" {
		writeError(w, 400, "invalid member binding")
		return
	}
	cid, ok := parseUUIDOrBadRequest(w, v.ConnectionID, "connection id")
	if !ok {
		return
	}
	uid, ok := parseUUIDOrBadRequest(w, v.PatchbayUserID, "user id")
	if !ok {
		return
	}
	id := parseUUID(uuid.NewString())
	_, err := h.DB.Exec(r.Context(), `INSERT INTO linear_member_binding(id,workspace_id,connection_id,patchbay_user_id,linear_user_id) VALUES($1,$2,$3,$4,$5) ON CONFLICT(workspace_id,patchbay_user_id) DO UPDATE SET connection_id=EXCLUDED.connection_id,linear_user_id=EXCLUDED.linear_user_id,updated_at=now()`, id, ws, cid, uid, v.LinearUserID)
	if err != nil {
		writeError(w, 409, "Linear user is already mapped")
		return
	}
	var savedID, savedWorkspaceID, savedConnectionID, savedUserID pgtype.UUID
	var savedLinearUserID string
	var createdAt, updatedAt time.Time
	err = h.DB.QueryRow(r.Context(), `SELECT id,workspace_id,connection_id,patchbay_user_id,linear_user_id,created_at,updated_at FROM linear_member_binding WHERE workspace_id=$1 AND patchbay_user_id=$2`, ws, uid).Scan(
		&savedID, &savedWorkspaceID, &savedConnectionID, &savedUserID, &savedLinearUserID, &createdAt, &updatedAt,
	)
	if err != nil {
		writeError(w, 500, "failed to load Linear member binding")
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"id": uuidToString(savedID), "workspace_id": uuidToString(savedWorkspaceID),
		"connection_id": uuidToString(savedConnectionID), "patchbay_user_id": uuidToString(savedUserID),
		"linear_user_id": savedLinearUserID, "created_at": createdAt.Format(time.RFC3339Nano),
		"updated_at": updatedAt.Format(time.RFC3339Nano),
	})
}
func (h *Handler) DeleteLinearMemberBinding(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) {
		return
	}
	ws, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	uid, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "userId"), "user id")
	if !ok {
		return
	}
	_, err := h.DB.Exec(r.Context(), `DELETE FROM linear_member_binding WHERE workspace_id=$1 AND patchbay_user_id=$2`, ws, uid)
	if err != nil {
		writeError(w, 500, "failed to delete Linear member binding")
		return
	}
	w.WriteHeader(204)
}

func (h *Handler) DryRunLinearBinding(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) {
		return
	}
	ws, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	var v linearBindingRequest
	if json.NewDecoder(r.Body).Decode(&v) != nil || v.PatchbayProjectID == "" || v.LinearProjectID == "" {
		writeError(w, 400, "invalid dry-run request")
		return
	}
	pid, ok := parseUUIDOrBadRequest(w, v.PatchbayProjectID, "project id")
	if !ok {
		return
	}
	var local int
	if err := h.DB.QueryRow(r.Context(), `SELECT count(*) FROM issue WHERE workspace_id=$1 AND project_id=$2`, ws, pid).Scan(&local); err != nil {
		writeError(w, 500, "failed to count local issues")
		return
	}
	writeJSON(w, 200, map[string]any{"patchbay_project_id": v.PatchbayProjectID, "linear_project_id": v.LinearProjectID, "sync_mode": v.SyncMode, "initial_source_of_truth": v.InitialSourceOfTruth, "local_issue_count": local, "remote_issue_count": 0, "remote_issue_count_truncated": true, "candidate_import_count": 0, "candidate_publish_count": local, "unmapped_remote_status_count": 0, "exact_link_counts_available": false})
}
func (h *Handler) QueueLinearInitialImport(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) {
		return
	}
	ws, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	bid, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "bindingId"), "binding id")
	if !ok {
		return
	}
	var cid pgtype.UUID
	if err := h.DB.QueryRow(r.Context(), `SELECT connection_id FROM linear_project_binding WHERE id=$1 AND workspace_id=$2 AND status='active'`, bid, ws).Scan(&cid); err != nil {
		writeError(w, 404, "active Linear binding not found")
		return
	}
	id := parseUUID(uuid.NewString())
	payload, _ := json.Marshal(map[string]string{"binding_id": uuidToString(bid), "kind": "initial_import"})
	_, err := h.DB.Exec(r.Context(), `INSERT INTO linear_sync_inbox(id,connection_id,delivery_id,event_type,payload) VALUES($1,$2,$3,'initial_import',$4)`, id, cid, "initial-import:"+uuidToString(bid), payload)
	if err != nil {
		writeError(w, 500, "failed to queue Linear import")
		return
	}
	writeJSON(w, 202, map[string]any{"queued": true, "inbox_id": uuidToString(id)})
}

func (h *Handler) ListLinearSyncConflicts(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) {
		return
	}
	ws, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	status := r.URL.Query().Get("status")
	if status == "" {
		status = "open"
	}
	rows, err := h.DB.Query(r.Context(), `SELECT c.id,c.workspace_id,c.binding_id,c.link_id,c.patchbay_issue_id,c.linear_issue_id,l.linear_identifier,c.field,c.base_value,c.local_value,c.remote_value,c.source_event_id,c.source_event_at_ms,c.status,c.resolution,c.resolved_value,c.resolved_by_id,c.created_at,c.updated_at FROM linear_sync_conflict c LEFT JOIN linear_issue_link l ON l.id=c.link_id WHERE c.workspace_id=$1 AND c.status=$2 ORDER BY c.created_at DESC`, ws, status)
	if err != nil {
		writeError(w, 500, "failed to list Linear conflicts")
		return
	}
	defer rows.Close()
	out := []map[string]any{}
	for rows.Next() {
		var id, wid, bid, lid, pid pgtype.UUID
		var linear, field, event, status string
		var identifier, resolution pgtype.Text
		var base, local, remote, resolved []byte
		var at pgtype.Int8
		var resolvedBy pgtype.UUID
		var created, updated time.Time
		if rows.Scan(&id, &wid, &bid, &lid, &pid, &linear, &identifier, &field, &base, &local, &remote, &event, &at, &status, &resolution, &resolved, &resolvedBy, &created, &updated) != nil {
			writeError(w, 500, "failed to read Linear conflicts")
			return
		}
		var bv, lv, rv, resv any
		_ = json.Unmarshal(base, &bv)
		_ = json.Unmarshal(local, &lv)
		_ = json.Unmarshal(remote, &rv)
		if resolved != nil {
			_ = json.Unmarshal(resolved, &resv)
		}
		out = append(out, map[string]any{"id": uuidToString(id), "workspace_id": uuidToString(wid), "binding_id": uuidToString(bid), "link_id": uuidToString(lid), "patchbay_issue_id": uuidToString(pid), "linear_issue_id": linear, "linear_identifier": textToPtr(identifier), "field": field, "base_value": bv, "local_value": lv, "remote_value": rv, "source_event_id": event, "source_event_at_ms": int8ToPtr(at), "status": status, "resolution": textToPtr(resolution), "resolved_value": resv, "resolved_by_id": uuidToPtr(resolvedBy), "created_at": created.Format(time.RFC3339Nano), "updated_at": updated.Format(time.RFC3339Nano)})
	}
	writeJSON(w, 200, map[string]any{"conflicts": out})
}
func (h *Handler) ResolveLinearSyncConflict(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) {
		return
	}
	ws, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	cid, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "conflictId"), "conflict id")
	if !ok {
		return
	}
	var v struct {
		Resolution  string `json:"resolution"`
		ManualValue any    `json:"manual_value"`
	}
	if json.NewDecoder(r.Body).Decode(&v) != nil || (v.Resolution != "local" && v.Resolution != "remote" && v.Resolution != "manual") {
		writeError(w, 400, "invalid conflict resolution")
		return
	}
	member, _ := middleware.MemberFromContext(r.Context())
	manual, _ := json.Marshal(v.ManualValue)
	tag, err := h.DB.Exec(r.Context(), `UPDATE linear_sync_conflict SET status='resolved',resolution=$3,resolved_value=CASE WHEN $3='manual' THEN $4 ELSE CASE WHEN $3='local' THEN local_value ELSE remote_value END END,resolved_by_id=$5,updated_at=now() WHERE id=$1 AND workspace_id=$2 AND status='open'`, cid, ws, v.Resolution, manual, member.UserID)
	if err != nil {
		writeError(w, 500, "failed to resolve Linear conflict")
		return
	}
	if tag.RowsAffected() == 0 {
		writeError(w, 404, "open Linear conflict not found")
		return
	}
	writeJSON(w, 200, map[string]any{"id": uuidToString(cid), "workspace_id": uuidToString(ws), "status": "resolved", "resolution": v.Resolution, "resolved_value": v.ManualValue})
}

func (h *Handler) HandleLinearWebhook(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) {
		return
	}
	if !h.linearConfigured() {
		writeError(w, 503, "Linear webhook is not configured")
		return
	}
	body, err := io.ReadAll(io.LimitReader(r.Body, 2<<20))
	if err != nil {
		writeError(w, 400, "invalid webhook body")
		return
	}
	expected := hmac.New(sha256.New, []byte(h.LinearWebhookSecret))
	_, _ = expected.Write(body)
	sig, err := hex.DecodeString(strings.TrimSpace(r.Header.Get("Linear-Signature")))
	if err != nil || !hmac.Equal(sig, expected.Sum(nil)) {
		writeError(w, 401, "invalid Linear signature")
		return
	}
	var event struct {
		Type, Action     string
		OrganizationID   string `json:"organizationId"`
		WebhookTimestamp int64  `json:"webhookTimestamp"`
		Data             struct {
			ID string `json:"id"`
		} `json:"data"`
	}
	if json.Unmarshal(body, &event) != nil || event.OrganizationID == "" {
		writeError(w, 400, "invalid Linear webhook payload")
		return
	}
	var cid pgtype.UUID
	if err := h.DB.QueryRow(r.Context(), `SELECT id FROM linear_connection WHERE organization_id=$1 AND status='active'`, event.OrganizationID).Scan(&cid); err != nil {
		writeError(w, 404, "Linear connection not found")
		return
	}
	delivery := strings.TrimSpace(r.Header.Get("Linear-Delivery"))
	if delivery == "" {
		sum := sha256.Sum256(body)
		delivery = hex.EncodeToString(sum[:])
	}
	id := parseUUID(uuid.NewString())
	eventType := event.Type + ":" + event.Action
	tag, err := h.DB.Exec(r.Context(), `INSERT INTO linear_sync_inbox(id,connection_id,delivery_id,event_type,payload) VALUES($1,$2,$3,$4,$5) ON CONFLICT(connection_id,delivery_id) DO NOTHING`, id, cid, delivery, eventType, body)
	if err != nil {
		writeError(w, 500, "failed to persist Linear webhook")
		return
	}
	if tag.RowsAffected() > 0 && h.LinearWorker != nil {
		h.LinearWorker.Wake()
	}
	writeJSON(w, 202, map[string]any{"accepted": true, "duplicate": tag.RowsAffected() == 0})
}

func (h *Handler) DisconnectLinear(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) {
		return
	}
	ws, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	if h.LinearWorker != nil {
		var sealed []byte
		if queryErr := h.DB.QueryRow(r.Context(), `SELECT access_token_encrypted FROM linear_connection WHERE workspace_id=$1 AND status<>'revoked' ORDER BY created_at DESC LIMIT 1`, ws).Scan(&sealed); queryErr == nil {
			token, openErr := h.LinearSecretBox.Open(sealed)
			if openErr != nil {
				writeError(w, 500, "failed to open Linear token for revocation")
				return
			}
			if revokeErr := h.LinearWorker.api.RevokeToken(r.Context(), string(token), h.LinearClientID, h.LinearClientSecret); revokeErr != nil {
				writeError(w, 502, "Linear token revocation failed")
				return
			}
		} else if !errors.Is(queryErr, pgx.ErrNoRows) {
			writeError(w, 500, "failed to load Linear connection")
			return
		}
	}
	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		writeError(w, 500, "failed to disconnect Linear")
		return
	}
	defer tx.Rollback(r.Context())
	queries := []string{`DELETE FROM linear_sync_conflict WHERE workspace_id=$1`, `DELETE FROM linear_sync_outbox WHERE workspace_id=$1`, `DELETE FROM linear_issue_link WHERE workspace_id=$1`, `DELETE FROM linear_member_binding WHERE workspace_id=$1`, `DELETE FROM linear_sync_inbox WHERE connection_id IN (SELECT id FROM linear_connection WHERE workspace_id=$1)`, `DELETE FROM linear_project_binding WHERE workspace_id=$1`, `DELETE FROM linear_oauth_state WHERE workspace_id=$1`, `DELETE FROM linear_connection WHERE workspace_id=$1`}
	for _, q := range queries {
		if _, err = tx.Exec(r.Context(), q, ws); err != nil {
			writeError(w, 500, "failed to disconnect Linear")
			return
		}
	}
	if err = tx.Commit(r.Context()); err != nil {
		writeError(w, 500, "failed to disconnect Linear")
		return
	}
	w.WriteHeader(204)
}
