package handler

import (
	"context"
	"os"
	"testing"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/jackc/pgx/v5/pgxpool"

	dbfx "github.com/patchbay-ai/patchbay/server/internal/testutil"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
)

func TestConnectionStatusOwnershipAndPublicProjectionDB(t *testing.T) {
	dsn := os.Getenv("DATABASE_URL")
	if dsn == "" {
		t.Skip("integration test requires DATABASE_URL")
	}
	ctx := context.Background()
	pool, err := pgxpool.New(ctx, dsn)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(pool.Close)
	fx := dbfx.New(pool, "", "")
	suffix := util.UUIDToString(dbid.NewV7())
	fx.UserID = fx.User(t, "Connection status", suffix+"@example.test")
	fx.WorkspaceID = fx.Workspace(t, "Connection status", "connection-"+suffix)
	workspaceID := util.MustParseUUID(fx.WorkspaceID)
	q := db.New(pool)
	h := &Handler{Queries: q}
	for _, provider := range []string{"slack", "feishu", "dingtalk", "telegram", "wecom", "weixin"} {
		t.Run(provider, func(t *testing.T) {
			id := dbid.NewV7()
			_, err := pool.Exec(ctx, `INSERT INTO channel_installation
				(id, workspace_id, agent_id, channel_type, config, installer_user_id, ws_lease_token, ws_lease_expires_at)
				VALUES ($1, $2, $3, $4, '{}', $5, 'current', now() + interval '5 minutes')`,
				id, workspaceID, pgtype.UUID{Valid: true}, provider, util.MustParseUUID(fx.UserID))
			if err != nil {
				t.Fatal(err)
			}
			fx.Cleanup(t, "DELETE FROM channel_installation WHERE id = $1", id)
			fx.Cleanup(t, "DELETE FROM channel_installation_runtime_observation WHERE installation_id = $1", id)
			claim := func(token string, want int64) {
				t.Helper()
				count, err := q.ClaimChannelRuntimeObserver(ctx, db.ClaimChannelRuntimeObserverParams{InstallationID: id, ObserverToken: token})
				if err != nil || count != want {
					t.Fatalf("claim %s: count=%d err=%v", token, count, err)
				}
			}
			observe := func(token string, want int64) {
				t.Helper()
				count, err := q.ObserveChannelRuntime(ctx, db.ObserveChannelRuntimeParams{
					InstallationID: id, ObserverToken: token, State: "healthy", ErrorSummary: "credential-sentinel",
				})
				if err != nil || count != want {
					t.Fatalf("observe %s: count=%d err=%v", token, count, err)
				}
			}
			claim("current", 1)
			observe("current", 1)
			ids := []string{uuidToString(id)}
			statuses, err := h.loadConnectionStatuses(ctx, workspaceID, ids)
			if err != nil || statuses[ids[0]].State != "healthy" || statuses[ids[0]].ErrorSummary != nil {
				t.Fatalf("public projection did not expose only safe confirmed state: %+v %v", statuses, err)
			}
			if _, err := pool.Exec(ctx, "UPDATE channel_installation SET ws_lease_token = 'successor' WHERE id = $1", id); err != nil {
				t.Fatal(err)
			}
			claim("successor", 1)
			claim("current", 0)
			observe("current", 0)
			observe("successor", 1)
			for _, condition := range []string{"hosted_paused_at = now()", "hosted_paused_at = NULL, status = 'revoked'"} {
				if _, err := pool.Exec(ctx, "UPDATE channel_installation SET "+condition+" WHERE id = $1", id); err != nil {
					t.Fatal(err)
				}
				claim("successor", 0)
				observe("successor", 0)
				statuses, err = h.loadConnectionStatuses(ctx, workspaceID, ids)
				if err != nil || statuses[ids[0]].State != "offline" {
					t.Fatalf("paused/revoked connection was reported online: %+v %v", statuses, err)
				}
			}
			outside, err := h.loadConnectionStatuses(ctx, dbid.NewV7(), ids)
			if err != nil || outside[ids[0]].ObservedAt != nil || outside[ids[0]].State != "offline" {
				t.Fatalf("status escaped its workspace: %+v %v", outside, err)
			}
		})
	}
}
