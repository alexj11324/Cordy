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
	linearapi "github.com/patchbay-ai/patchbay/server/internal/integrations/linear"
	"github.com/patchbay-ai/patchbay/server/internal/middleware"
)

const (
	linearAuthorizeURL = linearapi.DefaultAuthorizeURL
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
	return h.LinearSecretBox != nil && strings.TrimSpace(h.LinearClientID) != "" && strings.TrimSpace(h.LinearClientSecret) != ""
}

func (h *Handler) linearWebhookConfigured() bool {
	return h.linearConfigured() && strings.TrimSpace(h.LinearWebhookSecret) != ""
}

func (h *Handler) linearAPI() linearapi.API {
	if h.LinearWorker != nil && h.LinearWorker.api != nil {
		return h.LinearWorker.api
	}
	return linearapi.NewHTTPClient(nil)
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
	var active bool
	if err := h.DB.QueryRow(r.Context(), `SELECT EXISTS(SELECT 1 FROM linear_connection WHERE workspace_id=$1 AND status='active')`, ws).Scan(&active); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to inspect Linear connection")
		return
	}
	if active {
		writeError(w, http.StatusConflict, "Linear is already connected")
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
	_, _ = h.DB.Exec(r.Context(), `DELETE FROM linear_oauth_state WHERE expires_at<now() OR consumed_at<now()-interval '1 day'`)
	_, err = h.DB.Exec(r.Context(), `INSERT INTO linear_oauth_state
(id,state_hash,workspace_id,user_id,code_verifier_encrypted,redirect_uri,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7)`,
		parseUUID(uuid.NewString()), hex.EncodeToString(stateSum[:]), ws, member.UserID, sealed, callback, expires)
	if err != nil {
		writeError(w, 500, "failed to store OAuth state")
		return
	}
	q := url.Values{"client_id": {h.LinearClientID}, "redirect_uri": {callback}, "response_type": {"code"},
		"scope": {linearapi.OAuthScope}, "state": {state}, "code_challenge": {base64.RawURLEncoding.EncodeToString(verifierSum[:])},
		"code_challenge_method": {"S256"}, "actor": {"app"}}
	writeJSON(w, http.StatusOK, map[string]any{"authorization_url": linearAuthorizeURL + "?" + q.Encode(), "state_expires_at": expires.Format(time.RFC3339Nano)})
}

func (h *Handler) LinearOAuthCallback(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) || !h.linearConfigured() {
		return
	}
	code, state := strings.TrimSpace(r.URL.Query().Get("code")), strings.TrimSpace(r.URL.Query().Get("state"))
	if code == "" || state == "" {
		writeError(w, http.StatusBadRequest, "code and state are required")
		return
	}
	stateSum := sha256.Sum256([]byte(state))
	consumeTx, err := h.TxStarter.Begin(r.Context())
	if err != nil { writeError(w, http.StatusInternalServerError, "failed to begin OAuth callback"); return }
	var ws, user pgtype.UUID
	var sealed []byte
	var redirect string
	err = consumeTx.QueryRow(r.Context(), `UPDATE linear_oauth_state SET consumed_at=now() WHERE state_hash=$1 AND consumed_at IS NULL AND expires_at>now() RETURNING workspace_id,user_id,code_verifier_encrypted,redirect_uri`, hex.EncodeToString(stateSum[:])).Scan(&ws, &user, &sealed, &redirect)
	if errors.Is(err, pgx.ErrNoRows) { _ = consumeTx.Rollback(r.Context()); writeError(w, http.StatusBadRequest, "OAuth state is invalid, expired, or already used"); return }
	if err != nil { _ = consumeTx.Rollback(r.Context()); writeError(w, http.StatusInternalServerError, "failed to consume OAuth state"); return }
	if err = consumeTx.Commit(r.Context()); err != nil { writeError(w, http.StatusInternalServerError, "failed to commit OAuth state"); return }
	if providerError := strings.TrimSpace(r.URL.Query().Get("error")); providerError != "" {
		linearRedirect(w, r, redirect, "error", providerError)
		return
	}
	verifier, err := h.LinearSecretBox.Open(sealed)
	if err != nil { writeError(w, http.StatusInternalServerError, "failed to open OAuth state"); return }
	if role := h.linearWorkspaceRole(r.Context(), ws, user); role != "owner" && role != "admin" {
		linearRedirect(w, r, redirect, "error", "workspace membership changed")
		return
	}
	// The active-row check is repeated in the write transaction below. This
	// early check avoids exchanging a provider code when a parallel callback
	// already installed the workspace, while the row lock is the authority.
	if active, checkErr := h.linearWorkspaceHasActiveConnection(r.Context(), ws); checkErr != nil { writeError(w, http.StatusInternalServerError, "failed to inspect Linear connection"); return } else if active { linearRedirect(w, r, redirect, "already_connected", "1"); return }
	token, err := h.linearAPI().ExchangeAuthorizationCode(r.Context(), code, redirect, string(verifier), h.LinearClientID, h.LinearClientSecret)
	if err != nil { writeError(w, http.StatusBadGateway, "Linear token exchange failed"); return }
	identity, err := h.linearAPI().DiscoverIdentity(r.Context(), token.AccessToken)
	if err != nil { writeError(w, http.StatusBadGateway, "failed to load Linear organization"); return }
	access, err := h.LinearSecretBox.Seal([]byte(token.AccessToken)); if err != nil { writeError(w, 500, "failed to encrypt Linear token"); return }
	refresh, err := h.LinearSecretBox.Seal([]byte(token.RefreshToken)); if err != nil { writeError(w, 500, "failed to encrypt Linear token"); return }
	scopeValues := strings.FieldsFunc(token.Scope, func(r rune) bool { return r == ',' || r == ' ' || r == '\t' })
	scopes, _ := json.Marshal(scopeValues)
	writeTx, err := h.TxStarter.Begin(r.Context()); if err != nil { writeError(w, 500, "failed to save Linear connection"); return }
	defer writeTx.Rollback(r.Context())
	var lockedStatus string
	if lockErr := writeTx.QueryRow(r.Context(), `SELECT status FROM linear_connection WHERE workspace_id=$1 ORDER BY created_at DESC LIMIT 1 FOR UPDATE`, ws).Scan(&lockedStatus); lockErr == nil && lockedStatus == "active" { linearRedirect(w, r, redirect, "already_connected", "1"); return }
	_, err = writeTx.Exec(r.Context(), `INSERT INTO linear_connection(id,workspace_id,organization_id,organization_name,actor_id,access_token_encrypted,refresh_token_encrypted,token_expires_at,scopes,status,created_by_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'active',$10) ON CONFLICT(workspace_id) DO UPDATE SET organization_id=EXCLUDED.organization_id,organization_name=EXCLUDED.organization_name,actor_id=EXCLUDED.actor_id,access_token_encrypted=EXCLUDED.access_token_encrypted,refresh_token_encrypted=EXCLUDED.refresh_token_encrypted,token_expires_at=EXCLUDED.token_expires_at,scopes=EXCLUDED.scopes,status='active',last_error=NULL,updated_at=now()`, parseUUID(uuid.NewString()), ws, identity.OrganizationID, identity.OrganizationName, identity.ActorID, access, refresh, time.Now().UTC().Add(token.ExpiresIn), scopes, user)
	if err != nil { writeError(w, 500, "failed to save Linear connection"); return }
	if err := writeTx.Commit(r.Context()); err != nil { writeError(w, 500, "failed to commit Linear connection"); return }
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

func linearRedirect(w http.ResponseWriter, r *http.Request, target, key, value string) {
	u, err := url.Parse(target)
	if err != nil || u.Scheme == "" || u.Host == "" { writeJSON(w, http.StatusOK, map[string]string{key: value}); return }
	query := u.Query(); query.Set(key, value); u.RawQuery = query.Encode(); http.Redirect(w, r, u.String(), http.StatusFound)
}

func (h *Handler) linearWorkspaceRole(ctx context.Context, workspaceID, userID pgtype.UUID) string {
	var role string
	_ = h.DB.QueryRow(ctx, `SELECT role FROM member WHERE workspace_id=$1 AND user_id=$2`, workspaceID, userID).Scan(&role)
	return role
}

func (h *Handler) linearWorkspaceHasActiveConnection(ctx context.Context, workspaceID pgtype.UUID) (bool, error) {
	var active bool
	err := h.DB.QueryRow(ctx, `SELECT EXISTS(SELECT 1 FROM linear_connection WHERE workspace_id=$1 AND status='active')`, workspaceID).Scan(&active)
	return active, err
}

func (h *Handler) linearAccessToken(ctx context.Context, ws pgtype.UUID) (string, error) {
	if h.LinearWorker != nil {
		var connectionID pgtype.UUID
		if err := h.DB.QueryRow(ctx, `SELECT id FROM linear_connection WHERE workspace_id=$1 AND status='active' ORDER BY created_at DESC LIMIT 1`, ws).Scan(&connectionID); err == nil {
			return h.LinearWorker.accessToken(ctx, connectionID)
		}
	}
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
	catalog, err := h.linearAPI().Catalog(r.Context(), token)
	if err != nil {
		writeError(w, 502, "failed to load Linear catalog")
		return
	}
	teams := make([]map[string]any, 0, len(catalog.Teams))
	for _, team := range catalog.Teams { teams = append(teams, map[string]any{"id": team.ID, "key": team.Key, "name": team.Name, "organization_id": team.OrganizationID}) }
	projects := make([]map[string]any, 0, len(catalog.ProjectCatalog))
	for _, project := range catalog.ProjectCatalog { projects = append(projects, map[string]any{"id": project.ID, "name": project.Name, "team_id": project.TeamID}) }
	states := make([]map[string]any, 0, len(catalog.States))
	for _, state := range catalog.States { states = append(states, map[string]any{"id": state.ID, "name": state.Name, "type": state.Type, "color": state.Color, "team_id": state.TeamID}) }
	users := make([]map[string]any, 0, len(catalog.Users))
	for _, user := range catalog.Users { users = append(users, map[string]any{"id": user.ID, "name": user.Name, "email": user.Email, "active": user.Active}) }
	labels := make([]map[string]any, 0, len(catalog.Labels))
	for _, label := range catalog.Labels { parent, team := any(nil), any(nil); if label.ParentID != "" { parent = label.ParentID }; if label.TeamID != "" { team = label.TeamID }; labels = append(labels, map[string]any{"id": label.ID, "name": label.Name, "color": label.Color, "is_group": label.IsGroup, "parent_id": parent, "team_id": team}) }
	writeJSON(w, 200, map[string]any{"teams": teams, "projects": projects, "states": states, "users": users, "labels": labels})
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
	if v.Status == "" {
		v.Status = "draft"
	}
	switch v.SyncMode { case "import", "publish", "two_way", "not_synced": default: return false }
	switch v.Status {
	case "draft", "active", "paused", "tombstone":
	default:
		return false
	}
	if v.ConnectionID == "" || v.PatchbayProjectID == "" || v.LinearProjectID == "" { return false }
	if v.Status == "active" || v.Status == "paused" {
		if v.SyncMode == "not_synced" || v.LinearTeamID == nil || strings.TrimSpace(*v.LinearTeamID) == "" { return false }
	}
	if v.SyncMode == "not_synced" && v.InitialSourceOfTruth != nil { return false }
	if v.SyncMode == "import" && v.Status != "draft" && (v.InitialSourceOfTruth == nil || *v.InitialSourceOfTruth != "linear") { return false }
	if v.SyncMode == "publish" && v.Status != "draft" && (v.InitialSourceOfTruth == nil || *v.InitialSourceOfTruth != "patchbay") { return false }
	if v.SyncMode == "two_way" && v.Status != "draft" && (v.InitialSourceOfTruth == nil || (*v.InitialSourceOfTruth != "linear" && *v.InitialSourceOfTruth != "patchbay")) { return false }
	return v.Status != "tombstone"
}

func linearJSONMap(value map[string]any) []byte {
	if value == nil { return []byte(`{}`) }
	b, err := json.Marshal(value); if err != nil { return []byte(`{}`) }; return b
}

func (h *Handler) validateLinearBindingScope(ctx context.Context, ws, cid, pid pgtype.UUID, remoteProject string, team *string, status string) error {
	var connectionWorkspace pgtype.UUID
	var connectionStatus string
	if err := h.DB.QueryRow(ctx, `SELECT workspace_id,status FROM linear_connection WHERE id=$1`, cid).Scan(&connectionWorkspace, &connectionStatus); err != nil { return err }
	if connectionWorkspace != ws || connectionStatus != "active" { return errors.New("Linear connection is not active in this workspace") }
	var projectExists bool
	if err := h.DB.QueryRow(ctx, `SELECT EXISTS(SELECT 1 FROM project WHERE id=$1 AND workspace_id=$2)`, pid, ws).Scan(&projectExists); err != nil { return err }
	if !projectExists { return errors.New("Patchbay project is not in this workspace") }
	if status == "active" || status == "paused" {
		if team == nil || strings.TrimSpace(*team) == "" { return errors.New("Linear team is required for an active binding") }
		token, err := h.linearAccessToken(ctx, ws); if err != nil { return err }
		if err := h.linearAPI().ValidateBinding(ctx, token, remoteProject, *team); err != nil { return err }
	}
	return nil
}

func (h *Handler) seedLinearOutbound(ctx context.Context, tx pgx.Tx, ws, bindingID, projectID pgtype.UUID) error {
	if _, err := tx.Exec(ctx, `INSERT INTO linear_sync_outbox(id,workspace_id,binding_id,issue_id,event_key,event_type,payload) SELECT gen_random_uuid(),i.workspace_id,$2::uuid,i.id,'binding-seed:'||$2::uuid::text||':'||i.id::text||':'||i.revision::text,'issue_updated',jsonb_build_object('id',i.id,'revision',i.revision) FROM issue i WHERE i.workspace_id=$1::uuid AND i.project_id=$3::uuid AND i.status<>'cancelled' ON CONFLICT(binding_id,event_key) DO NOTHING`, ws, bindingID, projectID); err != nil { return err }
	if _, err := tx.Exec(ctx, `INSERT INTO linear_comment_link(workspace_id,binding_id,issue_id,comment_id,linear_comment_id,origin) SELECT c.workspace_id,$2::uuid,c.issue_id,c.id,gen_random_uuid()::text,'patchbay' FROM comment c JOIN issue i ON i.id=c.issue_id AND i.workspace_id=c.workspace_id WHERE c.workspace_id=$1::uuid AND i.project_id=$3::uuid AND i.status<>'cancelled' AND c.author_type IN ('member','agent') AND c.type='comment' ON CONFLICT(binding_id,comment_id) DO NOTHING`, ws, bindingID, projectID); err != nil { return err }
	_, err := tx.Exec(ctx, `WITH RECURSIVE candidates AS (SELECT c.* FROM comment c JOIN issue i ON i.id=c.issue_id AND i.workspace_id=c.workspace_id WHERE c.workspace_id=$1::uuid AND i.project_id=$3::uuid AND i.status<>'cancelled' AND c.author_type IN ('member','agent') AND c.type='comment'), ordered AS (SELECT c.*,0 AS depth FROM candidates c WHERE c.parent_id IS NULL OR NOT EXISTS (SELECT 1 FROM candidates parent WHERE parent.id=c.parent_id) UNION ALL SELECT child.*,parent.depth+1 FROM candidates child JOIN ordered parent ON child.parent_id=parent.id) INSERT INTO linear_sync_outbox(id,workspace_id,binding_id,issue_id,event_key,event_type,payload,created_at) SELECT gen_random_uuid(),c.workspace_id,$2::uuid,c.issue_id,'binding-seed-comment:'||$2::uuid::text||':'||c.id::text||':'||c.revision::text,'comment_created',jsonb_build_object('comment_id',c.id,'body',c.content,'parent_id',CASE WHEN EXISTS (SELECT 1 FROM candidates parent WHERE parent.id=c.parent_id) THEN c.parent_id ELSE NULL END,'author_type',c.author_type,'author_id',c.author_id),transaction_timestamp()+((c.depth+1)*interval '1 millisecond') FROM ordered c ON CONFLICT(binding_id,event_key) DO NOTHING`, ws, bindingID, projectID)
	return err
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
	if err := h.validateLinearBindingScope(r.Context(), ws, cid, pid, v.LinearProjectID, v.LinearTeamID, v.Status); err != nil { writeError(w, http.StatusConflict, err.Error()); return }
	sm, am := linearJSONMap(v.StatusMapping), linearJSONMap(v.AgentLabelMapping)
	id := parseUUID(uuid.NewString())
	tx, err := h.TxStarter.Begin(r.Context()); if err != nil { writeError(w, 500, "failed to create Linear binding"); return }; defer tx.Rollback(r.Context())
	_, err = tx.Exec(r.Context(), `INSERT INTO linear_project_binding(id,workspace_id,connection_id,patchbay_project_id,linear_project_id,linear_team_id,status,sync_mode,initial_source_of_truth,status_mapping,agent_label_mapping,activated_at,paused_at,created_by_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,CASE WHEN $7='active' THEN now() END,CASE WHEN $7='paused' THEN now() END,$12)`, id, ws, cid, pid, v.LinearProjectID, v.LinearTeamID, v.Status, v.SyncMode, v.InitialSourceOfTruth, sm, am, member.UserID)
	if err != nil {
		writeError(w, 409, "Linear binding conflicts with an existing mapping")
		return
	}
	if v.Status == "active" && (v.SyncMode == "publish" || v.SyncMode == "two_way") { if err = h.seedLinearOutbound(r.Context(), tx, ws, id, pid); err != nil { writeError(w, 500, "failed to seed Linear outbound sync"); return } }
	if err = tx.Commit(r.Context()); err != nil { writeError(w, 500, "failed to commit Linear binding"); return }
	b, err := scanLinearBinding(h.DB.QueryRow(r.Context(), linearBindingSelect+` WHERE id=$1 AND workspace_id=$2`, id, ws))
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
	if v.Status == "" { v.Status = "draft" }
	var current struct { ConnectionID, PatchbayProjectID pgtype.UUID; LinearProjectID string; Status, SyncMode string; LinearTeamID pgtype.Text }
	if err := h.DB.QueryRow(r.Context(), `SELECT connection_id,patchbay_project_id,linear_project_id,status,sync_mode,linear_team_id FROM linear_project_binding WHERE id=$1 AND workspace_id=$2`, bid, ws).Scan(&current.ConnectionID,&current.PatchbayProjectID,&current.LinearProjectID,&current.Status,&current.SyncMode,&current.LinearTeamID); errors.Is(err, pgx.ErrNoRows) { writeError(w, 404, "Linear binding not found"); return } else if err != nil { writeError(w, 500, "failed to load Linear binding"); return }
	if current.ConnectionID != cid || current.PatchbayProjectID != pid || current.LinearProjectID != v.LinearProjectID { writeError(w, http.StatusConflict, "Linear binding identifiers are immutable"); return }
	if err := h.validateLinearBindingScope(r.Context(), ws, cid, pid, v.LinearProjectID, v.LinearTeamID, v.Status); err != nil { writeError(w, http.StatusConflict, err.Error()); return }
	sm, am := linearJSONMap(v.StatusMapping), linearJSONMap(v.AgentLabelMapping)
	tx, err := h.TxStarter.Begin(r.Context()); if err != nil { writeError(w, 500, "failed to update Linear binding"); return }; defer tx.Rollback(r.Context())
	tag, err := tx.Exec(r.Context(), `UPDATE linear_project_binding SET linear_team_id=$3,status=$4,sync_mode=$5,initial_source_of_truth=$6,status_mapping=$7,agent_label_mapping=$8,activated_at=CASE WHEN $4='active' THEN COALESCE(activated_at,now()) ELSE activated_at END,paused_at=CASE WHEN $4='paused' THEN now() ELSE paused_at END,updated_at=now() WHERE id=$1 AND workspace_id=$2`, bid, ws, v.LinearTeamID, v.Status, v.SyncMode, v.InitialSourceOfTruth, sm, am)
	if err != nil { writeError(w, 409, "Linear binding conflicts with an existing mapping"); return }
	if tag.RowsAffected() == 0 { writeError(w, 404, "Linear binding not found"); return }
	if current.Status != "active" && v.Status == "active" && (v.SyncMode == "publish" || v.SyncMode == "two_way") { if err = h.seedLinearOutbound(r.Context(), tx, ws, bid, pid); err != nil { writeError(w, 500, "failed to seed Linear outbound sync"); return } }
	if err = tx.Commit(r.Context()); err != nil { writeError(w, 500, "failed to commit Linear binding"); return }
	b, err := scanLinearBinding(h.DB.QueryRow(r.Context(), linearBindingSelect+` WHERE id=$1 AND workspace_id=$2`, bid, ws))
	if err != nil { writeError(w, 500, "failed to load Linear binding"); return }
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
	tag, err := tx.Exec(r.Context(), `WITH tombstoned AS (UPDATE linear_project_binding SET status='tombstone',paused_at=COALESCE(paused_at,now()),updated_at=now() WHERE id=$1 AND workspace_id=$2 RETURNING id), deleted_conflicts AS (DELETE FROM linear_sync_conflict WHERE binding_id IN (SELECT id FROM tombstoned) AND workspace_id=$2), deleted_outbox AS (DELETE FROM linear_sync_outbox WHERE binding_id IN (SELECT id FROM tombstoned) AND workspace_id=$2) UPDATE linear_issue_link SET sync_status='deleted',updated_at=now() WHERE binding_id IN (SELECT id FROM tombstoned) AND workspace_id=$2`, bid, ws)
	if err != nil { writeError(w, 500, "failed to delete Linear binding"); return }
	if tag.RowsAffected() == 0 { writeError(w, 404, "Linear binding not found"); return }
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
	rows, err := h.DB.Query(r.Context(), `SELECT mb.id,mb.workspace_id,mb.connection_id,mb.patchbay_user_id,mb.linear_user_id,mb.created_at,mb.updated_at FROM linear_member_binding mb JOIN linear_connection c ON c.id=mb.connection_id AND c.workspace_id=mb.workspace_id WHERE mb.workspace_id=$1 AND c.status='active' ORDER BY mb.created_at`, ws)
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
	var connectionWorkspace pgtype.UUID
	var connectionStatus string
	if err := h.DB.QueryRow(r.Context(), `SELECT workspace_id,status FROM linear_connection WHERE id=$1`, cid).Scan(&connectionWorkspace, &connectionStatus); err != nil || connectionWorkspace != ws || connectionStatus != "active" { writeError(w, http.StatusConflict, "active Linear connection not found in this workspace"); return }
	var memberWorkspace pgtype.UUID
	if err := h.DB.QueryRow(r.Context(), `SELECT workspace_id FROM member WHERE workspace_id=$1 AND user_id=$2`, ws, uid).Scan(&memberWorkspace); err != nil || memberWorkspace != ws { writeError(w, http.StatusConflict, "Patchbay member is not in this workspace"); return }
	id := parseUUID(uuid.NewString())
	_, err := h.DB.Exec(r.Context(), `INSERT INTO linear_member_binding(id,workspace_id,connection_id,patchbay_user_id,linear_user_id) VALUES($1,$2,$3,$4,$5) ON CONFLICT(workspace_id,connection_id,patchbay_user_id) DO UPDATE SET linear_user_id=EXCLUDED.linear_user_id,updated_at=now()`, id, ws, cid, uid, v.LinearUserID)
	if err != nil {
		writeError(w, 409, "Linear user is already mapped")
		return
	}
	var savedID, savedWorkspaceID, savedConnectionID, savedUserID pgtype.UUID
	var savedLinearUserID string
	var createdAt, updatedAt time.Time
	err = h.DB.QueryRow(r.Context(), `SELECT id,workspace_id,connection_id,patchbay_user_id,linear_user_id,created_at,updated_at FROM linear_member_binding WHERE workspace_id=$1 AND connection_id=$2 AND patchbay_user_id=$3`, ws, cid, uid).Scan(
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
	_, err := h.DB.Exec(r.Context(), `DELETE FROM linear_member_binding WHERE workspace_id=$1 AND patchbay_user_id=$2 AND connection_id IN (SELECT id FROM linear_connection WHERE workspace_id=$1 AND status='active')`, ws, uid)
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
	var local int64
	if err := h.DB.QueryRow(r.Context(), `SELECT count(*) FROM issue WHERE workspace_id=$1 AND project_id=$2`, ws, pid).Scan(&local); err != nil {
		writeError(w, 500, "failed to count local issues")
		return
	}
	remote := linearapi.DryRunCounts{}
	if v.SyncMode != "not_synced" {
		token, err := h.linearAccessToken(r.Context(), ws); if err != nil { writeError(w, http.StatusConflict, "Linear connection requires authorization"); return }
		remote, err = h.linearAPI().DryRunCounts(r.Context(), token, v.LinearProjectID, linearValueOrEmpty(v.LinearTeamID), v.StatusMapping); if err != nil { writeError(w, http.StatusBadGateway, "failed to preview Linear project"); return }
	}
	candidateImport, candidatePublish := int64(0), int64(0)
	if v.SyncMode == "import" || v.SyncMode == "two_way" { candidateImport = int64(remote.RemoteIssues) }
	if v.SyncMode == "publish" || v.SyncMode == "two_way" { candidatePublish = local }
	writeJSON(w, 200, map[string]any{"patchbay_project_id": v.PatchbayProjectID, "linear_project_id": v.LinearProjectID, "sync_mode": v.SyncMode, "initial_source_of_truth": v.InitialSourceOfTruth, "local_issue_count": local, "remote_issue_count": remote.RemoteIssues, "remote_issue_count_truncated": remote.Truncated, "candidate_import_count": candidateImport, "candidate_publish_count": candidatePublish, "unmapped_remote_status_count": remote.UnmappedStatuses, "exact_link_counts_available": false})
}

func linearValueOrEmpty(value *string) string { if value == nil { return "" }; return *value }
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
	var mode string
	if err := h.DB.QueryRow(r.Context(), `SELECT connection_id,sync_mode FROM linear_project_binding WHERE id=$1 AND workspace_id=$2 AND status='active'`, bid, ws).Scan(&cid, &mode); err != nil {
		writeError(w, 404, "active Linear binding not found")
		return
	}
	if mode != "import" && mode != "two_way" { writeError(w, http.StatusConflict, "Linear binding does not pull from Linear"); return }
	id := parseUUID(uuid.NewString())
	payload, _ := json.Marshal(map[string]string{"binding_id": uuidToString(bid), "kind": "initial_import"})
	tag, err := h.DB.Exec(r.Context(), `INSERT INTO linear_sync_inbox(id,connection_id,delivery_id,event_type,payload) VALUES($1,$2,$3,'initial_import',$4) ON CONFLICT(connection_id,delivery_id) DO NOTHING`, id, cid, "initial-import:"+uuidToString(bid), payload)
	if err != nil {
		writeError(w, 500, "failed to queue Linear import")
		return
	}
	writeJSON(w, 202, map[string]any{"queued": tag.RowsAffected() > 0, "inbox_id": uuidToString(id)})
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
	var raw map[string]json.RawMessage
	if json.NewDecoder(r.Body).Decode(&raw) != nil { writeError(w, 400, "invalid conflict resolution"); return }
	var resolution string
	if err := json.Unmarshal(raw["resolution"], &resolution); err != nil || (resolution != "local" && resolution != "remote" && resolution != "manual") {
		writeError(w, 400, "invalid conflict resolution")
		return
	}
	member, _ := middleware.MemberFromContext(r.Context())
	var manual any
	if resolution == "manual" {
		value, ok := raw["manual_value"]; if !ok { writeError(w, 400, "manual_value is required"); return }
		if err := json.Unmarshal(value,&manual); err != nil { writeError(w,400,"invalid manual_value"); return }
	}
	conflict, _, err := h.resolveLinearConflict(r.Context(),ws,cid,member.UserID,resolution,manual)
	if errors.Is(err,pgx.ErrNoRows) { writeError(w,404,"open Linear conflict not found"); return }
	if err != nil { writeError(w,http.StatusConflict,err.Error()); return }
	var resolved any; _ = json.Unmarshal(conflict.ResolvedValue,&resolved)
	writeJSON(w,200,map[string]any{"id":uuidToString(conflict.ID),"workspace_id":uuidToString(conflict.WorkspaceID),"binding_id":uuidToString(conflict.BindingID),"link_id":uuidToString(conflict.LinkID),"patchbay_issue_id":uuidToString(conflict.PatchbayIssueID),"linear_issue_id":conflict.LinearIssueID,"field":conflict.Field,"status":conflict.Status,"resolution":conflict.Resolution.String,"resolved_value":resolved,"resolved_by_id":uuidToString(conflict.ResolvedByID)})
}

type linearWebhookEvent struct {
	Type, Action       string
	OrganizationID     string `json:"organizationId"`
	WebhookID         string `json:"webhookId"`
	WebhookTimestamp  int64  `json:"webhookTimestamp"`
	Data              struct { ID string `json:"id"` } `json:"data"`
}

func validateLinearWebhook(secret string, headers http.Header, body []byte, now time.Time) (linearWebhookEvent, string, error) {
	var event linearWebhookEvent
	signature, err := hex.DecodeString(strings.TrimSpace(headers.Get("Linear-Signature")))
	if err != nil { return event, "", errors.New("invalid Linear signature encoding") }
	mac := hmac.New(sha256.New, []byte(secret)); _, _ = mac.Write(body)
	if !hmac.Equal(signature, mac.Sum(nil)) { return event, "", errors.New("invalid Linear signature") }
	if err := json.Unmarshal(body, &event); err != nil { return event, "", err }
	if strings.TrimSpace(event.OrganizationID) == "" || strings.TrimSpace(event.WebhookID) == "" || event.WebhookTimestamp <= 0 { return event, "", errors.New("Linear webhook omitted organizationId, webhookId, or webhookTimestamp") }
	if timestampHeader := strings.TrimSpace(headers.Get("Linear-Timestamp")); timestampHeader != "" {
		parsed, parseErr := linearapi.ParseInt64(timestampHeader); if parseErr != nil || parsed != event.WebhookTimestamp { return event, "", errors.New("Linear webhook timestamps do not match") }
	}
	if delta := now.UnixMilli() - event.WebhookTimestamp; delta > 60_000 || delta < -60_000 { return event, "", errors.New("Linear webhook timestamp is stale") }
	delivery := strings.TrimSpace(headers.Get("Linear-Delivery")); if delivery == "" { delivery = linearapi.SHA256Hex(string(body)) }
	return event, delivery, nil
}

func (h *Handler) HandleLinearWebhook(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) { return }
	if !h.linearWebhookConfigured() { writeError(w, http.StatusServiceUnavailable, "Linear webhook is not configured"); return }
	body, err := io.ReadAll(io.LimitReader(r.Body, 2<<20+1)); if err != nil || len(body) > 2<<20 { writeError(w, http.StatusRequestEntityTooLarge, "invalid Linear webhook body"); return }
	event, delivery, err := validateLinearWebhook(h.LinearWebhookSecret, r.Header, body, time.Now().UTC())
	if err != nil {
		if strings.Contains(err.Error(), "signature") { writeError(w, http.StatusUnauthorized, err.Error()) } else { writeError(w, http.StatusBadRequest, err.Error()) }
		return
	}
	tx, err := h.TxStarter.Begin(r.Context()); if err != nil { writeError(w, 500, "failed to persist Linear webhook"); return }; defer tx.Rollback(r.Context())
	rows, err := tx.Query(r.Context(), `SELECT id,webhook_id FROM linear_connection WHERE organization_id=$1 AND status='active' AND (webhook_id=$2 OR webhook_id IS NULL) ORDER BY (webhook_id=$2) DESC,created_at DESC FOR UPDATE`, event.OrganizationID, event.WebhookID)
	if err != nil { writeError(w, 500, "failed to find Linear connection"); return }
	type candidate struct { id pgtype.UUID; webhook pgtype.Text }; candidates := []candidate{}
	for rows.Next() { var item candidate; if scanErr := rows.Scan(&item.id,&item.webhook); scanErr != nil { rows.Close(); writeError(w, 500, "failed to read Linear connection"); return }; candidates = append(candidates,item) }
	rows.Close(); if err = rows.Err(); err != nil { writeError(w, 500, "failed to read Linear connection"); return }
	if len(candidates) != 1 { writeError(w, http.StatusNotFound, "Linear webhook connection not found"); return }
	cid := candidates[0].id
	if !candidates[0].webhook.Valid { tag, bindErr := tx.Exec(r.Context(), `UPDATE linear_connection SET webhook_id=$2,updated_at=now() WHERE id=$1 AND status='active' AND webhook_id IS NULL`, cid, event.WebhookID); if bindErr != nil || tag.RowsAffected() != 1 { writeError(w, http.StatusConflict, "Linear webhook binding changed"); return } }
	eventType := strings.TrimSpace(r.Header.Get("Linear-Event")); if eventType == "" { eventType = strings.TrimSpace(event.Type+":"+event.Action); if eventType == ":" { eventType = "unknown" } }
	tag, err := tx.Exec(r.Context(), `INSERT INTO linear_sync_inbox(id,connection_id,delivery_id,event_type,payload) VALUES($1,$2,$3,$4,$5) ON CONFLICT(connection_id,delivery_id) DO NOTHING`, parseUUID(uuid.NewString()), cid, delivery, eventType, body)
	if err != nil { writeError(w, 500, "failed to persist Linear webhook"); return }
	if _, err = tx.Exec(r.Context(), `UPDATE linear_connection SET last_success_at=now(),last_error=NULL,updated_at=now() WHERE id=$1 AND status='active'`, cid); err != nil { writeError(w, 500, "failed to update Linear webhook health"); return }
	if err = tx.Commit(r.Context()); err != nil { writeError(w, 500, "failed to commit Linear webhook"); return }
	if tag.RowsAffected() > 0 && h.LinearWorker != nil { h.LinearWorker.Wake() }
	writeJSON(w, http.StatusOK, map[string]any{"accepted": true, "duplicate": tag.RowsAffected() == 0})
}

func (h *Handler) DisconnectLinear(w http.ResponseWriter, r *http.Request) {
	if !h.requireLinear(w, r) {
		return
	}
	ws, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "workspace id")
	if !ok {
		return
	}
	var connectionID pgtype.UUID
	var status string
	var sealed []byte
	if err := h.DB.QueryRow(r.Context(), `SELECT id,status,access_token_encrypted FROM linear_connection WHERE workspace_id=$1 ORDER BY created_at DESC LIMIT 1`, ws).Scan(&connectionID,&status,&sealed); errors.Is(err, pgx.ErrNoRows) { w.WriteHeader(http.StatusNoContent); return } else if err != nil { writeError(w, 500, "failed to load Linear connection"); return }
	if status == "active" {
		token, openErr := h.LinearSecretBox.Open(sealed)
		if openErr != nil { writeError(w, 500, "failed to open Linear token for revocation"); return }
		if revokeErr := h.linearAPI().RevokeToken(r.Context(), string(token), h.LinearClientID, h.LinearClientSecret); revokeErr != nil {
			if !linearapi.IsKind(revokeErr, linearapi.ErrorInvalidGrant) { writeError(w, http.StatusBadGateway, "Linear token revocation failed"); return }
		}
	}
	tx, err := h.TxStarter.Begin(r.Context()); if err != nil { writeError(w, 500, "failed to disconnect Linear"); return }; defer tx.Rollback(r.Context())
	if _, err = tx.Exec(r.Context(), `UPDATE linear_connection SET status='revoked',last_error=NULL,updated_at=now() WHERE id=$1 AND workspace_id=$2`, connectionID, ws); err != nil { writeError(w, 500, "failed to disconnect Linear"); return }
	if err = tx.Commit(r.Context()); err != nil { writeError(w, 500, "failed to commit Linear disconnect"); return }
	w.WriteHeader(204)
}
