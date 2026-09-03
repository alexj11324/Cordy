package hostedcapacity

import (
	"context"
	"os"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// capacityTestDB mirrors the repo's DB-test contract: skip (not fail) when
// DATABASE_URL is absent or the schema has not been migrated.
func capacityTestDB(t *testing.T) *pgxpool.Pool {
	t.Helper()
	dsn := os.Getenv("DATABASE_URL")
	if dsn == "" {
		dsn = "postgres://patchbay:patchbay@localhost:5432/patchbay?sslmode=disable"
	}
	ctx := context.Background()
	pool, err := pgxpool.New(ctx, dsn)
	if err != nil {
		t.Skipf("no database: %v", err)
	}
	if err := pool.Ping(ctx); err != nil {
		pool.Close()
		t.Skipf("database not reachable: %v", err)
	}
	var migrated bool
	if err := pool.QueryRow(ctx, `
SELECT to_regclass('public.channel_installation') IS NOT NULL
   AND to_regclass('public.channel_installation_runtime_observation') IS NOT NULL
`).Scan(&migrated); err != nil || !migrated {
		pool.Close()
		t.Skip("capacity tables not present (database not migrated)")
	}
	t.Cleanup(pool.Close)
	return pool
}

// TestReconcileAgainstPostgres proves the generated capacity SQL against a
// real database: the FOR UPDATE list, the pause/resume updates, the runtime
// observation, and the work-finding filters that make a paused installation
// invisible.
func TestReconcileAgainstPostgres(t *testing.T) {
	pool := capacityTestDB(t)
	ctx := context.Background()

	const (
		workspaceID   = "c0ffee00-0000-4000-8000-000000000001"
		userID        = "c0ffee00-0000-4000-8000-000000000002"
		runtimeID     = "c0ffee00-0000-4000-8000-000000000003"
		agentA        = "c0ffee00-0000-4000-8000-000000000004"
		agentB        = "c0ffee00-0000-4000-8000-000000000005"
		agentC        = "c0ffee00-0000-4000-8000-000000000006"
		installationA = "c0ffee00-0000-4000-8000-000000000011"
		installationB = "c0ffee00-0000-4000-8000-000000000012"
		installationC = "c0ffee00-0000-4000-8000-000000000013"
	)
	suffix := time.Now().UTC().Format("150405.000000000")

	exec := func(query string, args ...any) {
		t.Helper()
		if _, err := pool.Exec(ctx, query, args...); err != nil {
			t.Fatalf("seed capacity fixture: %v", err)
		}
	}
	cleanup := func() {
		_, _ = pool.Exec(ctx, `DELETE FROM channel_installation_runtime_observation WHERE installation_id IN ($1, $2, $3)`,
			installationA, installationB, installationC)
		_, _ = pool.Exec(ctx, `DELETE FROM channel_installation WHERE workspace_id = $1`, workspaceID)
		_, _ = pool.Exec(ctx, `DELETE FROM agent WHERE workspace_id = $1`, workspaceID)
		_, _ = pool.Exec(ctx, `DELETE FROM agent_runtime WHERE workspace_id = $1`, workspaceID)
		_, _ = pool.Exec(ctx, `DELETE FROM workspace WHERE id = $1`, workspaceID)
		_, _ = pool.Exec(ctx, `DELETE FROM "user" WHERE id = $1`, userID)
	}
	cleanup()
	t.Cleanup(cleanup)

	exec(`INSERT INTO "user" (id, name, email) VALUES ($1, 'Hosted capacity test', $2)`, userID, "capacity-"+suffix+"@patchbay.test")
	exec(`INSERT INTO workspace (id, name, slug, description) VALUES ($1, 'Hosted capacity test', $2, '')`, workspaceID, "hosted-capacity-"+suffix)
	exec(`INSERT INTO agent_runtime (id, workspace_id, name, runtime_mode, provider) VALUES ($1, $2, 'Capacity runtime', 'local', 'patchbay_daemon')`, runtimeID, workspaceID)
	// One installation per agent — the real shape of a multi-bot workspace
	// (channel_installation is unique on (workspace, agent, channel_type)).
	for i, agent := range []string{agentA, agentB, agentC} {
		exec(`INSERT INTO agent (id, workspace_id, name, runtime_mode, runtime_id, kind) VALUES ($1, $2, $3, 'local', $4, 'user')`,
			agent, workspaceID, "Capacity agent "+string(rune('A'+i)), runtimeID)
	}
	// Oldest-first created_at makes the reconcile keep-order deterministic.
	for i, row := range []struct {
		installation string
		agent        string
	}{
		{installationA, agentA},
		{installationB, agentB},
		{installationC, agentC},
	} {
		exec(`
			INSERT INTO channel_installation (
				id, workspace_id, agent_id, channel_type, config, installer_user_id, status, created_at
			) VALUES ($1, $2, $3, 'slack', $4::jsonb, $5, 'active', now() - ($6 || ' minutes')::interval)
		`, row.installation, workspaceID, row.agent, `{"app_id":"capacity-app-`+row.installation+`"}`, userID, string(rune('0'+3-i)))
	}

	queries := db.New(pool)
	ws := util.MustParseUUID(workspaceID)
	agent := util.MustParseUUID(agentA)

	// Cap of 2 with 3 active installations: the newest is paused, not revoked.
	result, err := Reconcile(ctx, dbQueries{queries}, pool, ws, ptrOf(int64(2)))
	if err != nil {
		t.Fatalf("Reconcile(cap 2): %v", err)
	}
	if len(result.Paused) != 1 || result.Paused[0] != util.MustParseUUID(installationC) {
		t.Fatalf("paused = %+v, want only the newest installation", result.Paused)
	}
	var (
		pausedAt  *time.Time
		statusRow string
	)
	if err := pool.QueryRow(ctx,
		`SELECT hosted_paused_at, status FROM channel_installation WHERE id = $1`, installationC,
	).Scan(&pausedAt, &statusRow); err != nil {
		t.Fatalf("read paused installation: %v", err)
	}
	if pausedAt == nil || statusRow != "active" {
		t.Fatalf("paused installation: hosted_paused_at=%v status=%s, want a pause marker on an active row", pausedAt, statusRow)
	}
	var observationState, observationCode, observer string
	if err := pool.QueryRow(ctx, `
SELECT state, error_code, observer_token
  FROM channel_installation_runtime_observation WHERE installation_id = $1
`, installationC).Scan(&observationState, &observationCode, &observer); err != nil {
		t.Fatalf("read pause observation: %v", err)
	}
	if observationState != pausedState || observationCode != pausedReason || observer != ObserverToken {
		t.Fatalf("observation = %s/%s/%s, want offline/hosted_quota_paused/%s", observationState, observationCode, observer, ObserverToken)
	}
	// The work-finding filters must not see the paused row.
	visible, err := queries.ListActiveChannelInstallations(ctx, "slack")
	if err != nil {
		t.Fatalf("ListActiveChannelInstallations: %v", err)
	}
	if len(visible) != 2 {
		t.Fatalf("visible installations = %d, want 2 (paused row filtered)", len(visible))
	}

	// Admission at the cap: a reconnect of the SAME slot is allowed, a new
	// slot is refused — both under the real workspace lock.
	qtx := dbQueries{queries}
	if err := AdmitInstall(ctx, qtx, ws, "slack", agent, ptrOf(int64(2))); err != nil {
		t.Fatalf("AdmitInstall(same slot at cap) = %v, want allowed", err)
	}
	// A fourth agent with no installation models a genuinely new slot.
	fourthAgent := "c0ffee00-0000-4000-8000-000000000014"
	exec(`INSERT INTO agent (id, workspace_id, name, runtime_mode, runtime_id, kind) VALUES ($1, $2, 'Capacity agent D', 'local', $3, 'user')`,
		fourthAgent, workspaceID, runtimeID)
	if err := AdmitInstall(ctx, qtx, ws, "slack", util.MustParseUUID(fourthAgent), ptrOf(int64(2))); err != ErrLimitReached {
		t.Fatalf("AdmitInstall(new slot at cap) = %v, want ErrLimitReached", err)
	}

	// Capacity returns: the paused installation is resumed and visible again.
	result, err = Reconcile(ctx, dbQueries{queries}, pool, ws, ptrOf(int64(3)))
	if err != nil {
		t.Fatalf("Reconcile(cap 3): %v", err)
	}
	if len(result.Resumed) != 1 || result.Resumed[0] != util.MustParseUUID(installationC) {
		t.Fatalf("resumed = %+v, want the paused installation", result.Resumed)
	}
	if err := pool.QueryRow(ctx,
		`SELECT hosted_paused_at FROM channel_installation WHERE id = $1`, installationC,
	).Scan(&pausedAt); err != nil || pausedAt != nil {
		t.Fatalf("resumed installation hosted_paused_at = %v, want NULL", pausedAt)
	}
	visible, err = queries.ListActiveChannelInstallations(ctx, "slack")
	if err != nil {
		t.Fatalf("ListActiveChannelInstallations after resume: %v", err)
	}
	if len(visible) != 3 {
		t.Fatalf("visible installations after resume = %d, want 3", len(visible))
	}
}
