package lark

import (
	"context"
	"errors"
	"os"
	"testing"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// channelScopeTestDB connects to the test Postgres (DATABASE_URL or the default
// local DSN, same as the handler suite) and returns a pool, or skips when no
// migrated database is reachable. Kept local to this file so the rest of the
// lark package stays DB-free.
func channelScopeTestDB(t *testing.T) *pgxpool.Pool {
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
	var present bool
	if err := pool.QueryRow(ctx, "SELECT to_regclass('public.channel_installation') IS NOT NULL").Scan(&present); err != nil || !present {
		pool.Close()
		t.Skip("channel_installation not present (database not migrated)")
	}
	t.Cleanup(pool.Close)
	return pool
}

// TestChannelStore_ScopesToFeishu is the MUL-3515 regression guard: the
// Lark/Feishu wrappers on ChannelStore must never read another channel_type's
// rows, even when a non-Feishu installation / chat-session binding / outbound
// card shares the same workspace, chat_session, or task. (Member-removal and
// chat-session cleanup deliberately stay all-channel; that is covered by the
// handler tests.)
func TestChannelStore_ScopesToFeishu(t *testing.T) {
	pool := channelScopeTestDB(t)
	ctx := context.Background()
	store := NewChannelStore(db.New(pool))

	// Synthetic, distinctive identifiers. channel_* has no foreign keys, so
	// these rows need no parent records and nothing cascades — which is also
	// why the test cleans up explicitly, by deterministic key, before and
	// after (a killed prior run must not leave colliding rows behind).
	const (
		feishuApp     = "cli_scope_feishu"
		slackApp      = "cli_scope_slack"
		wsID          = "5c09e000-0000-4000-8000-000000000001"
		agentID       = "5c09e000-0000-4000-8000-000000000002"
		chatSessionID = "5c09e000-0000-4000-8000-000000000003"
		taskID        = "5c09e000-0000-4000-8000-000000000004"
		installerID   = "5c09e000-0000-4000-8000-000000000005"
	)
	clean := func() {
		_, _ = pool.Exec(context.Background(),
			`DELETE FROM channel_installation WHERE config->>'app_id' = ANY($1)`,
			[]string{feishuApp, slackApp})
		_, _ = pool.Exec(context.Background(),
			`DELETE FROM channel_chat_session_binding WHERE chat_session_id = $1`, chatSessionID)
		_, _ = pool.Exec(context.Background(),
			`DELETE FROM channel_outbound_card_message WHERE task_id = $1`, taskID)
	}
	clean()
	t.Cleanup(clean)

	insertInstallation := func(channelType, app string) pgtype.UUID {
		var id string
		if err := pool.QueryRow(ctx, `
INSERT INTO channel_installation (workspace_id, agent_id, channel_type, config, installer_user_id)
VALUES ($1, $2, $3, jsonb_build_object('app_id', $4::text), $5)
RETURNING id
`, wsID, agentID, channelType, app, installerID).Scan(&id); err != nil {
			t.Fatalf("insert %s installation: %v", channelType, err)
		}
		return util.MustParseUUID(id)
	}
	feishuID := insertInstallation("feishu", feishuApp)
	slackID := insertInstallation("slack", slackApp)

	// A non-Feishu binding/card sharing this test's chat_session and task.
	if _, err := pool.Exec(ctx, `
INSERT INTO channel_chat_session_binding (chat_session_id, installation_id, channel_type, channel_chat_id, chat_type)
VALUES ($1, $2, 'slack', 'oc_scope_slack', 'p2p')
`, chatSessionID, slackID); err != nil {
		t.Fatalf("insert slack chat binding: %v", err)
	}
	if _, err := pool.Exec(ctx, `
INSERT INTO channel_outbound_card_message (chat_session_id, task_id, channel_type, channel_chat_id, channel_card_message_id, status)
VALUES ($1, $2, 'slack', 'oc_scope_slack', 'om_scope_slack', 'pending')
`, chatSessionID, taskID); err != nil {
		t.Fatalf("insert slack outbound card: %v", err)
	}

	wsUUID := util.MustParseUUID(wsID)
	sessionUUID := util.MustParseUUID(chatSessionID)
	taskUUID := util.MustParseUUID(taskID)

	// --- installation reads: Feishu visible, Slack invisible ---

	if got, err := store.GetLarkInstallation(ctx, feishuID); err != nil || got.AppID != feishuApp {
		t.Fatalf("GetLarkInstallation(feishu): got app=%q err=%v, want app=%q nil", got.AppID, err, feishuApp)
	}
	if _, err := store.GetLarkInstallation(ctx, slackID); !errors.Is(err, pgx.ErrNoRows) {
		t.Fatalf("GetLarkInstallation(slack): err=%v, want pgx.ErrNoRows (scoped out)", err)
	}

	if got, err := store.GetLarkInstallationInWorkspace(ctx, GetInstallationInWorkspaceParams{ID: feishuID, WorkspaceID: wsUUID}); err != nil || got.AppID != feishuApp {
		t.Fatalf("GetLarkInstallationInWorkspace(feishu): got app=%q err=%v, want app=%q nil", got.AppID, err, feishuApp)
	}
	if _, err := store.GetLarkInstallationInWorkspace(ctx, GetInstallationInWorkspaceParams{ID: slackID, WorkspaceID: wsUUID}); !errors.Is(err, pgx.ErrNoRows) {
		t.Fatalf("GetLarkInstallationInWorkspace(slack): err=%v, want pgx.ErrNoRows (scoped out)", err)
	}

	// list-by-workspace: only the Feishu installation in this workspace
	byWs, err := store.ListLarkInstallationsByWorkspace(ctx, wsUUID)
	if err != nil {
		t.Fatalf("ListLarkInstallationsByWorkspace: %v", err)
	}
	if len(byWs) != 1 || byWs[0].AppID != feishuApp {
		apps := make([]string, len(byWs))
		for i, r := range byWs {
			apps[i] = r.AppID
		}
		t.Fatalf("ListLarkInstallationsByWorkspace: got apps=%v, want exactly [%s]", apps, feishuApp)
	}

	// (ListConnectableLarkInstallations channel-type + live workspace/agent scoping
	// is covered by TestListConnectableLarkInstallations_SkipsOrphans in the handler
	// package, which has real workspace/agent fixtures the JOIN now requires.)

	// --- outbound reads: a Slack binding/card must not be seen as Feishu ---

	if _, err := store.GetLarkChatSessionBindingBySession(ctx, sessionUUID); !errors.Is(err, pgx.ErrNoRows) {
		t.Fatalf("GetLarkChatSessionBindingBySession(slack-bound session): err=%v, want pgx.ErrNoRows (scoped out)", err)
	}
	if _, err := store.GetLarkOutboundCardByTask(ctx, taskUUID); !errors.Is(err, pgx.ErrNoRows) {
		t.Fatalf("GetLarkOutboundCardByTask(slack card): err=%v, want pgx.ErrNoRows (scoped out)", err)
	}
}

func TestChannelInstallationStoreListsWorkspaceOwnedSocketInstallations(t *testing.T) {
	pool := channelScopeTestDB(t)
	ctx := context.Background()
	const (
		workspaceID = "5c09e200-0000-4000-8000-000000000001"
		orphanAgent = "5c09e200-0000-4000-8000-000000000002"
		installerID = "5c09e200-0000-4000-8000-000000000003"
		fixtureKey  = "workspace-owned-supervisor"
	)
	clean := func() {
		_, _ = pool.Exec(context.Background(),
			"DELETE FROM channel_installation WHERE config->>'scope_fixture' = $1", fixtureKey)
		_, _ = pool.Exec(context.Background(), "DELETE FROM workspace WHERE id = $1", workspaceID)
	}
	clean()
	t.Cleanup(clean)
	if _, err := pool.Exec(ctx, `
INSERT INTO workspace (id, name, slug, description)
VALUES ($1, 'Workspace-owned supervisor fixture', 'workspace-owned-supervisor-fixture', '')`, workspaceID); err != nil {
		t.Fatalf("insert workspace: %v", err)
	}

	rows, err := pool.Query(ctx, `
INSERT INTO channel_installation (
  workspace_id, agent_id, channel_type, config, installer_user_id
) VALUES
  ($1, NULL, 'feishu', jsonb_build_object('scope_fixture', $2::text, 'transport', 'long_connection'), $3),
  ($1, '00000000-0000-0000-0000-000000000000', 'telegram', jsonb_build_object('scope_fixture', $2::text), $3),
  ($1, $4, 'wecom', jsonb_build_object('scope_fixture', $2::text), $3),
  ($1, NULL, 'slack', jsonb_build_object('scope_fixture', $2::text, 'transport', 'webhook'), $3)
RETURNING id, channel_type`, workspaceID, fixtureKey, installerID, orphanAgent)
	if err != nil {
		t.Fatalf("insert installations: %v", err)
	}
	defer rows.Close()
	ids := make(map[string]string)
	for rows.Next() {
		var id, channelType string
		if err := rows.Scan(&id, &channelType); err != nil {
			t.Fatal(err)
		}
		ids[channelType] = id
	}
	if err := rows.Err(); err != nil {
		t.Fatal(err)
	}

	store := NewChannelInstallationStore(db.New(pool))
	installations, err := store.ListConnectableInstallations(ctx)
	if err != nil {
		t.Fatalf("ListConnectableInstallations: %v", err)
	}
	listed := make(map[string]struct{}, len(installations))
	for _, installation := range installations {
		listed[util.UUIDToString(installation.ID)] = struct{}{}
	}
	for _, channelType := range []string{"feishu", "telegram"} {
		if _, ok := listed[ids[channelType]]; !ok {
			t.Errorf("workspace-owned %s installation was omitted", channelType)
		}
	}
	for _, channelType := range []string{"wecom", "slack"} {
		if _, ok := listed[ids[channelType]]; ok {
			t.Errorf("%s installation must not be supervised", channelType)
		}
	}
}
