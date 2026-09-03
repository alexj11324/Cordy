package slack

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"sync"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/slack-go/slack"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel/engine"
	dbfx "github.com/patchbay-ai/patchbay/server/internal/testutil"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
)

// Only the task-execution boundary is fake: the real Slack resolvers, shared
// router, session persistence and Hub transactions run against PostgreSQL.
type captureHubTasks struct {
	engine.TaskEnqueuer
	mu     sync.Mutex
	agents []pgtype.UUID
}

func (t *captureHubTasks) EnqueueChannelChatTask(_ context.Context, session db.ChatSession, _ pgtype.UUID, _ bool, _ int64, _ pgtype.UUID, _ int64) (db.AgentTaskQueue, error) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.agents = append(t.agents, session.AgentID)
	return db.AgentTaskQueue{ID: dbid.NewV7()}, nil
}

func (t *captureHubTasks) captured() []pgtype.UUID {
	t.mu.Lock()
	defer t.mu.Unlock()
	return append([]pgtype.UUID(nil), t.agents...)
}

func TestManagedSlackHubRoutesAndSwitchesInPostgres(t *testing.T) {
	dsn := os.Getenv("DATABASE_URL")
	if dsn == "" {
		t.Skip("integration test requires DATABASE_URL")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	pool, err := pgxpool.New(ctx, dsn)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(pool.Close)
	fx := dbfx.New(pool, "", "")
	suffix := util.UUIDToString(dbid.NewV7())
	fx.UserID = fx.User(t, "Hub member", suffix+"@example.test")
	other := fx.User(t, "Other member", "other-"+suffix+"@example.test")
	fx.WorkspaceID = fx.Workspace(t, "Hub fixture", "hub-"+suffix)
	fx.Member(t, fx.WorkspaceID, fx.UserID, "owner")
	fx.Member(t, fx.WorkspaceID, other, "member")
	runtimeID := fx.Runtime(t, "Hub fixture runtime")
	first := util.MustParseUUID(fx.Agent(t, "Writer", runtimeID))
	second := util.MustParseUUID(fx.Agent(t, "Reviewer", runtimeID))
	private := util.MustParseUUID(fx.Agent(t, "Other private", runtimeID, dbfx.Cols{"owner_id": other}))
	appID, teamID := "APP-"+suffix, "TEAM-"+suffix
	config, err := json.Marshal(installConfig{AppID: ManagedRoutingKey(appID, teamID), ApiAppID: appID, TeamID: teamID, Transport: ManagedTransportWebhook})
	if err != nil {
		t.Fatal(err)
	}
	installationID := util.MustParseUUID(fx.Insert(t, "channel_installation", dbfx.Cols{
		"workspace_id": fx.WorkspaceID, "agent_id": "00000000-0000-0000-0000-000000000000",
		"channel_type": "slack", "config": config, "status": "active", "installer_user_id": fx.UserID,
	}))
	fx.Insert(t, "channel_user_binding", dbfx.Cols{
		"workspace_id": fx.WorkspaceID, "patchbay_user_id": fx.UserID,
		"installation_id": installationID, "channel_type": "slack", "channel_user_id": "U-fixture", "config": dbfx.Raw("'{}'::jsonb"),
	})
	t.Cleanup(func() {
		cleanupCtx, cleanupCancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cleanupCancel()
		for _, sql := range []string{
			`DELETE FROM channel_inbound_message_dedup WHERE installation_id = $1`,
			`DELETE FROM channel_chat_context_generation WHERE chat_session_id IN (SELECT chat_session_id FROM channel_chat_session_binding WHERE installation_id = $1)`,
			`DELETE FROM chat_message WHERE chat_session_id IN (SELECT chat_session_id FROM channel_chat_session_binding WHERE installation_id = $1)`,
			`DELETE FROM channel_chat_session_binding WHERE installation_id = $1`,
		} {
			if _, err := pool.Exec(cleanupCtx, sql, installationID); err != nil {
				t.Errorf("Hub fixture cleanup: %v", err)
			}
		}
		if _, err := pool.Exec(cleanupCtx, `DELETE FROM chat_session WHERE workspace_id = $1`, fx.WorkspaceID); err != nil {
			t.Errorf("Hub Chat cleanup: %v", err)
		}
	})
	q := db.New(pool)
	hub := engine.NewPostgresHubRouter(q, pool)
	tasks := &captureHubTasks{}
	router := engine.NewRouter(nil, tasks, q, engine.RouterConfig{Hub: hub})
	router.EnableRunBatching(time.Hour)
	router.Register(TypeSlack, NewSlackResolverSet(q, pool, nil, nil, nil))
	t.Cleanup(func() {
		cleanupCtx, cleanupCancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cleanupCancel()
		if !router.Drain(cleanupCtx) {
			t.Error("Hub router did not drain")
		}
	})
	raw, err := json.Marshal(slackRawEvent{APIAppID: appID, TeamID: teamID, EventType: "message"})
	if err != nil {
		t.Fatal(err)
	}
	message := func(id, text string) channel.InboundMessage {
		return channel.InboundMessage{
			EventID: id, MessageID: id, Text: text, CommandText: text, Type: channel.MsgTypeText, Raw: raw, AddressedToBot: true,
			Source: channel.Source{ChannelType: TypeSlack, ChatID: "D-fixture", ChatType: channel.ChatTypeP2P, SenderID: "U-fixture"},
		}
	}
	handle := func(id, text string) {
		t.Helper()
		if err := router.Handle(ctx, message(id, text)); err != nil {
			t.Fatalf("Hub input %s: %v", text, err)
		}
	}
	binding := func() db.ChannelChatSessionBinding {
		t.Helper()
		row, err := q.GetChannelChatSessionBinding(ctx, db.GetChannelChatSessionBindingParams{InstallationID: installationID, ChannelChatID: "D-fixture"})
		if err != nil {
			t.Fatal(err)
		}
		return row
	}
	handle("message-1", "First message")
	initial := binding()
	fx.Exec(t, `UPDATE chat_session SET session_id = 'old-provider-session', work_dir = '/old-agent-directory' WHERE id = $1`, initial.ChatSessionID)
	handle("command-1", "/agents 2")
	captured := tasks.captured()
	if len(captured) != 1 || captured[0] != first {
		t.Fatal("Agent switch did not flush the previous message to the original Agent")
	}
	session, err := q.GetChatSession(ctx, initial.ChatSessionID)
	if err != nil || session.AgentID != second || session.SessionID.Valid || session.WorkDir.Valid {
		t.Fatalf("Agent switch failed to clear old execution pointers: %v", err)
	}
	oldTask := util.MustParseUUID(fx.Task(t, util.UUIDToString(first), dbfx.Cols{
		"chat_session_id": initial.ChatSessionID, "runtime_id": runtimeID, "status": "cancelled",
		"session_id": "old-provider-session", "work_dir": "/old-agent-directory", "completed_at": dbfx.Raw("now()"),
		"channel_context_revision": 1,
	}))
	if err := q.AdvanceCancelledChatSessionPointer(ctx, oldTask); err != nil {
		t.Fatal(err)
	}
	if err := q.UpdateChatSessionSession(ctx, db.UpdateChatSessionSessionParams{
		ID: initial.ChatSessionID, AgentID: first, SessionID: pgtype.Text{String: "late-old-session", Valid: true},
		WorkDir: pgtype.Text{String: "/late-old-workdir", Valid: true}, RuntimeID: util.MustParseUUID(runtimeID),
	}); err != nil {
		t.Fatal(err)
	}
	session, err = q.GetChatSession(ctx, initial.ChatSessionID)
	if err != nil || session.SessionID.Valid || session.WorkDir.Valid {
		t.Fatal("an old Agent's late completion or cancellation restored its context after switching")
	}
	resume := db.GetLastChatTaskSessionParams{ChatSessionID: initial.ChatSessionID, AgentID: second, ChannelContextRevision: pgtype.Int8{Int64: 1, Valid: true}}
	if _, err := q.GetLastChatTaskSession(ctx, resume); !errors.Is(err, pgx.ErrNoRows) {
		t.Fatalf("same-runtime Agent borrowed another Agent's provider session: %v", err)
	}
	resume.AgentID = first
	if prior, err := q.GetLastChatTaskSession(ctx, resume); err != nil || prior.SessionID.String != "old-provider-session" {
		t.Fatal("Agent-scoped resume lost the old Agent's own valid history")
	}
	fx.Exec(t, `UPDATE chat_session SET session_id = 'current-provider-session', work_dir = '/current-agent-directory' WHERE id = $1`, initial.ChatSessionID)
	if err := q.ClearChatSessionSessionIfMatches(ctx, db.ClearChatSessionSessionIfMatchesParams{
		ID: initial.ChatSessionID, AgentID: first, SessionID: pgtype.Text{String: "current-provider-session", Valid: true}, RuntimeID: util.MustParseUUID(runtimeID),
	}); err != nil {
		t.Fatal(err)
	}
	handle("command-same-agent", "/agents 2")
	session, err = q.GetChatSession(ctx, initial.ChatSessionID)
	if err != nil || session.SessionID.String != "current-provider-session" || session.WorkDir.String != "/current-agent-directory" {
		t.Fatal("a stale failure or idempotent selection cleared the current Agent's execution pointers")
	}
	var stored map[string]string
	if err := json.Unmarshal(binding().Config, &stored); err != nil || stored["hub_agent_id"] != util.UUIDToString(second) || stored["channel_id"] != "D-fixture" {
		t.Fatal("Hub selection lost provider routing or did not commit")
	}
	handle("message-2", "Second message")
	if err := router.FlushPendingSession(ctx, initial.ChatSessionID); err != nil {
		t.Fatal(err)
	}
	captured = tasks.captured()
	if len(captured) != 2 || captured[1] != second {
		t.Fatal("the next ordinary message did not use the stored Agent")
	}
	if count := fx.Count(t, `SELECT count(*) FROM chat_message WHERE chat_session_id = $1`, initial.ChatSessionID); count != 2 {
		t.Fatalf("Hub control command became Agent input: message count=%d", count)
	}
	params := engine.HubPersistParams{Installation: engine.ResolvedInstallation{ID: installationID, WorkspaceID: util.MustParseUUID(fx.WorkspaceID)},
		UserID: util.MustParseUUID(fx.UserID), BindingKey: "missing-conversation", SessionID: initial.ChatSessionID, AgentID: first}
	if err := hub.PersistRoute(ctx, params); !errors.Is(err, engine.ErrRouteChanged) {
		t.Fatalf("missing binding did not roll back selection: %v", err)
	}
	session, err = q.GetChatSession(ctx, initial.ChatSessionID)
	if err != nil || session.AgentID != second {
		t.Fatal("failed binding update partially changed the Chat Agent")
	}
	params.BindingKey, params.AgentID = "D-fixture", private
	if err := hub.PersistRoute(ctx, params); !errors.Is(err, engine.ErrHubAgentUnavailable) {
		t.Fatalf("workspace admin invoked another member's private Agent: %v", err)
	}
	handle("command-2", "/new")
	next := binding()
	if next.ChatSessionID == initial.ChatSessionID {
		t.Fatal("/new did not create a new Chat generation")
	}
	if err := json.Unmarshal(next.Config, &stored); err != nil || stored["hub_agent_id"] != util.UUIDToString(second) {
		t.Fatal("/new did not carry the selected Hub Agent into its transaction")
	}
	processor := NewSlashCommandProcessor(SlashCommandConfig{
		Queries: q, Hub: hub, Control: NewSlackDMControlStarter(q, pool, tasks, nil, router.FlushPendingSession),
	})
	var reply string
	processor.respond = func(_ context.Context, _ string, text string) error { reply = text; return nil }
	command := slack.SlashCommand{Command: "/new", TriggerID: "managed-new-command", APIAppID: appID, TeamID: teamID,
		ChannelID: "D-fixture", UserID: "U-fixture", ResponseURL: "https://hooks.slack.test/fixture"}
	processor.Handle(ctx, command)
	newFromSlash := binding()
	if reply != slashNewStartedText || newFromSlash.ChatSessionID == next.ChatSessionID {
		t.Fatalf("managed /new did not use its trigger identity and Hub selection: %q", reply)
	}
	if err := json.Unmarshal(newFromSlash.Config, &stored); err != nil || stored["hub_agent_id"] != util.UUIDToString(second) {
		t.Fatal("managed /new lost the selected Agent")
	}
	processor.Handle(ctx, command)
	if binding().ChatSessionID != newFromSlash.ChatSessionID {
		t.Fatal("managed /new replay created another Chat")
	}
	command.Command, command.TriggerID = "/clear", "managed-clear-command"
	processor.Handle(ctx, command)
	if reply != slashClearStartedText || !binding().PendingFresh {
		t.Fatalf("managed /clear did not retain Hub selection and mark a fresh context: %q", reply)
	}
	fx.Exec(t, `UPDATE channel_installation SET hosted_paused_at = now() WHERE id = $1`, installationID)
	params.SessionID, params.AgentID = newFromSlash.ChatSessionID, first
	if err := hub.PersistRoute(ctx, params); err == nil {
		t.Fatal("paused installation accepted a late Agent switch")
	}
}
