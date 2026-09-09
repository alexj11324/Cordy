package devseed

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
)

// WakeRuntimeRegistration reuses the workspace update notification. Sending
// its current name preserves fixture edits while waking account daemons after
// Seed's direct database insert, without restarting any running tasks.
func WakeRuntimeRegistration(ctx context.Context, pool *pgxpool.Pool, origin, email string) error {
	var name string
	if err := pool.QueryRow(ctx, `SELECT name FROM workspace WHERE id = $1`, pgUUID(fixtureID("workspace"))).Scan(&name); err != nil {
		return err
	}
	return wakeRuntimeRegistration(ctx, origin, email, name)
}

func wakeRuntimeRegistration(ctx context.Context, origin, email, name string) error {
	parsed, err := url.Parse(origin)
	if err != nil || parsed.Scheme != "http" || (parsed.Hostname() != "localhost" && parsed.Hostname() != "127.0.0.1" && parsed.Hostname() != "::1") {
		return fmt.Errorf("runtime seed requires a loopback HTTP API")
	}
	client := &http.Client{Timeout: 15 * time.Second, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	call := func(path, method, token string, body any, target any) error {
		data, err := json.Marshal(body)
		if err != nil {
			return err
		}
		req, err := http.NewRequestWithContext(ctx, method, origin+path, bytes.NewReader(data))
		if err != nil {
			return err
		}
		req.Header.Set("Content-Type", "application/json")
		if token != "" {
			req.Header.Set("Authorization", "Bearer "+token)
		}
		resp, err := client.Do(req)
		if err != nil {
			return fmt.Errorf("development API unavailable")
		}
		defer resp.Body.Close()
		if resp.StatusCode < 200 || resp.StatusCode >= 300 {
			return fmt.Errorf("development API %s: HTTP %d", path, resp.StatusCode)
		}
		if target != nil {
			return json.NewDecoder(resp.Body).Decode(target)
		}
		return nil
	}
	var login struct {
		Token string `json:"token"`
	}
	if err := call("/auth/dev-login", http.MethodPost, "", map[string]string{"email": email, "onboarding": "keep"}, &login); err != nil {
		return err
	}
	if login.Token == "" {
		return fmt.Errorf("development login returned no token")
	}
	return call("/api/workspaces/"+fixtureID("workspace"), http.MethodPatch, login.Token, map[string]string{"name": name}, nil)
}

// SeedRuntimeAgents creates one starter Agent for each real online local
// runtime owned by the fixture user. Runtime-native default models are left
// unset: no hard-coded model names or fabricated runtime rows are necessary.
// Stable IDs and ON CONFLICT preserve renamed/edited/archived fixture Agents.
func SeedRuntimeAgents(ctx context.Context, pool *pgxpool.Pool, email string) (int64, error) {
	if email == "" {
		email = DefaultDeveloperEmail
	}
	rows, err := pool.Query(ctx, `SELECT r.id::text, r.name FROM agent_runtime r JOIN "user" u ON u.id = r.owner_id WHERE r.workspace_id = $1 AND u.email = $2 AND r.runtime_mode = 'local' AND r.status = 'online' AND r.daemon_id IS NOT NULL ORDER BY r.name`, pgUUID(fixtureID("workspace")), email)
	if err != nil {
		return 0, err
	}
	type runtime struct{ id, name string }
	var runtimes []runtime
	for rows.Next() {
		var r runtime
		if err := rows.Scan(&r.id, &r.name); err != nil {
			rows.Close()
			return 0, err
		}
		runtimes = append(runtimes, r)
	}
	rows.Close()
	if err := rows.Err(); err != nil {
		return 0, err
	}
	var inserted int64
	for _, r := range runtimes {
		result, err := pool.Exec(ctx, `INSERT INTO agent (id, workspace_id, name, description, runtime_mode, runtime_config, runtime_id, visibility, permission_mode, max_concurrent_tasks, owner_id, status)
  SELECT $1, r.workspace_id, $2, '开发样例智能体，使用当前设备 Harness 的默认模型。', r.runtime_mode, '{}'::jsonb, r.id, 'private', 'private', 1, r.owner_id, 'idle'
  FROM agent_runtime r WHERE r.id = $3 AND r.workspace_id = $4 AND r.status = 'online'
  ON CONFLICT DO NOTHING`, pgUUID(fixtureID("agent/runtime/"+r.id)), r.name, pgUUID(r.id), pgUUID(fixtureID("workspace")))
		if err != nil {
			return inserted, fmt.Errorf("seed runtime agent: %w", err)
		}
		inserted += result.RowsAffected()
	}
	return inserted, nil
}

// RuntimeIdentity is stable across a daemon's workspace registrations. Custom
// runtime profiles are workspace-scoped and are intentionally not inferred
// from another workspace's profile list.
type RuntimeIdentity struct {
	DaemonID  string
	Provider  string
	ProfileID string
}

// OnlineSeedRuntimes snapshots the actual installed runtime set. An empty
// workspaceID reads across the owner's workspaces before the new registration;
// passing the fixture workspace reads only its completed runtime rows.
func OnlineSeedRuntimes(ctx context.Context, pool *pgxpool.Pool, email, workspaceID string) ([]RuntimeIdentity, error) {
	if email == "" {
		email = DefaultDeveloperEmail
	}
	rows, err := pool.Query(ctx, `SELECT DISTINCT r.daemon_id, r.provider, COALESCE(r.profile_id::text, '')
 FROM agent_runtime r JOIN "user" u ON u.id = r.owner_id
 WHERE u.email = $1 AND r.runtime_mode = 'local' AND r.status = 'online'
 AND r.daemon_id IS NOT NULL
 AND (r.profile_id IS NULL OR r.workspace_id::text = $3)
 AND ($2::text = '' OR r.workspace_id::text = $2)`, email, workspaceID, fixtureID("workspace"))
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var result []RuntimeIdentity
	for rows.Next() {
		var r RuntimeIdentity
		if err := rows.Scan(&r.DaemonID, &r.Provider, &r.ProfileID); err != nil {
			return nil, err
		}
		result = append(result, r)
	}
	return result, rows.Err()
}

// RuntimeRegistrationComplete checks identities, not a positive count: the
// daemon may register several providers sequentially in one workspace sync.
func RuntimeRegistrationComplete(expected, registered []RuntimeIdentity) bool {
	if len(expected) == 0 {
		return false
	}
	available := make(map[RuntimeIdentity]bool, len(registered))
	for _, r := range registered {
		available[r] = true
	}
	for _, r := range expected {
		if !available[r] {
			return false
		}
	}
	return true
}
