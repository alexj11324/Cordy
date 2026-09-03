package slack

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/jackc/pgx/v5/pgxpool"

	dbfx "github.com/patchbay-ai/patchbay/server/internal/testutil"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
)

func TestManagedSlackCredentialLifecycleAndFencesDB(t *testing.T) {
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
	fx.UserID = fx.User(t, "Slack token test", suffix+"@example.test")
	fx.WorkspaceID = fx.Workspace(t, "Slack tokens", "slack-token-"+suffix)
	fx.Member(t, fx.WorkspaceID, fx.UserID, "owner")
	q := db.New(pool)
	box := testBox(t)
	install, err := NewInstallService(q, pool, box, nil)
	if err != nil {
		t.Fatal(err)
	}
	now := time.Date(2026, time.September, 3, 12, 0, 0, 0, time.UTC)
	expires := now.Add(time.Minute)
	params := RegisterManagedParams{WorkspaceID: util.MustParseUUID(fx.WorkspaceID), InstallerID: util.MustParseUUID(fx.UserID),
		Access: OAuthAccess{BotToken: "db-old-access", RefreshToken: "db-old-refresh", ExpiresAt: expires, AppID: "APP-" + suffix, TeamID: "TEAM-" + suffix}}
	row, err := install.RegisterManaged(ctx, params, nil)
	if err != nil {
		t.Fatal(err)
	}
	fx.Cleanup(t, `DELETE FROM channel_installation WHERE id = $1`, row.ID)
	fx.Cleanup(t, `DELETE FROM channel_installation_runtime_observation WHERE installation_id = $1`, row.ID)
	var initial installConfig
	if err := json.Unmarshal(row.Config, &initial); err != nil {
		t.Fatal(err)
	}
	listed, err := q.ListConnectableManagedSlackInstallations(ctx)
	if err != nil {
		t.Fatal(err)
	}
	found := false
	for _, item := range listed {
		if item.ID == row.ID {
			found = true
		}
	}
	if !found || row.AgentID != (pgtype.UUID{Valid: true}) {
		t.Fatal("workspace-owned managed installation is invisible to the worker")
	}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/oauth" {
			_, _ = w.Write([]byte(`{"ok":true,"access_token":"db-new-access","refresh_token":"db-new-refresh","expires_in":43200}`))
			return
		}
		if r.Header.Get("Authorization") != "Bearer db-new-access" {
			t.Error("database-backed rotation did not use the new token for its health check")
		}
		_, _ = w.Write([]byte(`{"ok":true}`))
	}))
	defer server.Close()
	oauth := &ManagedOAuthService{clientID: "client", clientSecret: "secret", tokenURL: server.URL + "/oauth", httpClient: server.Client()}
	worker := NewManagedTokenWorker(q, box, oauth, nil)
	worker.now = func() time.Time { return now }
	worker.authTestURL = server.URL + "/auth"
	// Never sweep other packages' fixtures in this shared CI database.
	worker.refreshInstallation(ctx, row)
	persisted, err := q.GetChannelInstallation(ctx, db.GetChannelInstallationParams{ID: row.ID, ChannelType: string(TypeSlack)})
	if err != nil {
		t.Fatal(err)
	}
	var current installConfig
	if err := json.Unmarshal(persisted.Config, &current); err != nil {
		t.Fatal(err)
	}
	refresh, err := decryptToken(current.RefreshTokenEncrypted, box.Open)
	if err != nil || refresh != "db-new-refresh" || current.AppID != initial.AppID || current.TeamID != initial.TeamID {
		t.Fatal("rotation lost the refresh credential or changed tenant identity")
	}
	health, err := q.GetRuntimeObservation(ctx, row.ID)
	if err != nil || health.State != "healthy" || health.ObserverToken != "managed:slack:webhook:v1" {
		t.Fatalf("health after rotation: state=%s err=%v", health.State, err)
	}
	rotation := db.RotateManagedSlackTokensParams{InstallationID: row.ID, PreviousRefreshToken: initial.RefreshTokenEncrypted,
		BotTokenEncrypted: initial.BotTokenEncrypted, RefreshTokenEncrypted: initial.RefreshTokenEncrypted,
		TokenExpiresAt: pgtype.Timestamptz{Time: expires, Valid: true}}
	if count, err := q.RotateManagedSlackTokens(ctx, rotation); err != nil || count != 0 {
		t.Fatalf("old refresh overwrote the new credential: count=%d err=%v", count, err)
	}
	observation := db.ObserveManagedSlackRuntimeParams{InstallationID: row.ID, ExpectedBotToken: initial.BotTokenEncrypted,
		State: "error", ErrorCode: "authentication_failed", ErrorSummary: "stale probe"}
	if count, err := q.ObserveManagedSlackRuntime(ctx, observation); err != nil || count != 0 {
		t.Fatalf("old probe overwrote new health: count=%d err=%v", count, err)
	}
	rotation.PreviousRefreshToken = current.RefreshTokenEncrypted
	observation.ExpectedBotToken = current.BotTokenEncrypted
	for _, state := range []struct {
		name string
		sql  string
	}{
		{"paused", `UPDATE channel_installation SET hosted_paused_at = now() WHERE id = $1`},
		{"revoked", `UPDATE channel_installation SET hosted_paused_at = NULL, status = 'revoked' WHERE id = $1`},
		{"BYO", `UPDATE channel_installation SET status = 'installed', config = jsonb_set(config, '{transport}', '"socket_mode"') WHERE id = $1`},
	} {
		fx.Exec(t, state.sql, row.ID)
		if count, err := q.RotateManagedSlackTokens(ctx, rotation); err != nil || count != 0 {
			t.Fatalf("%s accepted a refresh: count=%d err=%v", state.name, count, err)
		}
		if count, err := q.ObserveManagedSlackRuntime(ctx, observation); err != nil || count != 0 {
			t.Fatalf("%s accepted a health write: count=%d err=%v", state.name, count, err)
		}
		listed, err := q.ListConnectableManagedSlackInstallations(ctx)
		if err != nil {
			t.Fatal(err)
		}
		for _, item := range listed {
			if item.ID == row.ID {
				t.Fatalf("%s installation is still scheduled for provider work", state.name)
			}
		}
	}
	fx.Exec(t, `UPDATE channel_installation SET config = jsonb_set(config - 'bot_token_encrypted', '{transport}', '"webhook"') WHERE id = $1`, row.ID)
	observation.ExpectedBotToken = ""
	observation.ErrorCode = "credential_missing"
	if count, err := q.ObserveManagedSlackRuntime(ctx, observation); err != nil || count != 1 {
		t.Fatalf("missing credential was not reported: count=%d err=%v", count, err)
	}
	fx.Exec(t, `DELETE FROM channel_installation WHERE id = $1`, row.ID)
	if count, err := q.ObserveManagedSlackRuntime(ctx, observation); err != nil || count != 0 {
		t.Fatalf("deleted installation accepted a late probe: count=%d err=%v", count, err)
	}
}
