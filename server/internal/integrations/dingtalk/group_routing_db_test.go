package dingtalk

import (
	"context"
	"encoding/json"
	"errors"
	"testing"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgtype"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel/engine"
	dbfx "github.com/patchbay-ai/patchbay/server/internal/testutil"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
)

type groupRoutingFixture struct {
	fx        *dbfx.Fixture
	q         *db.Queries
	set       engine.ResolverSet
	inst      engine.ResolvedInstallation
	message   channel.InboundMessage
	other     pgtype.UUID
	finalizer *installationResolver
}

func newGroupRoutingFixture(t *testing.T) groupRoutingFixture {
	t.Helper()
	pool := dingtalkInstallTestDB(t)
	fx := dbfx.New(pool, "", "")
	suffix := util.UUIDToString(dbid.NewV7())
	fx.UserID = fx.User(t, "Group routing member", suffix+"@example.test")
	fx.WorkspaceID = fx.Workspace(t, "Group routing", "routing-"+suffix)
	fx.Member(t, fx.WorkspaceID, fx.UserID, "owner")
	first := fx.Agent(t, "Default agent", "")
	other := fx.Agent(t, "Group agent", "")
	config, err := json.Marshal(map[string]string{"app_id": suffix})
	if err != nil {
		t.Fatal(err)
	}
	installationID := fx.Insert(t, "channel_installation", dbfx.Cols{
		"workspace_id": fx.WorkspaceID, "agent_id": first, "channel_type": "dingtalk",
		"config": config, "installer_user_id": fx.UserID,
	})
	// Rows produced by the real pipeline belong to this isolated workspace.
	// Register parents first so cleanup removes their dependents first.
	fx.Cleanup(t, `DELETE FROM chat_session WHERE workspace_id = $1`, fx.WorkspaceID)
	fx.Cleanup(t, `DELETE FROM chat_message WHERE chat_session_id IN (SELECT id FROM chat_session WHERE workspace_id = $1)`, fx.WorkspaceID)
	fx.Cleanup(t, `DELETE FROM channel_chat_context_generation WHERE chat_session_id IN (SELECT id FROM chat_session WHERE workspace_id = $1)`, fx.WorkspaceID)
	fx.Cleanup(t, `DELETE FROM channel_chat_session_binding WHERE installation_id = $1`, installationID)
	fx.Cleanup(t, `DELETE FROM dingtalk_group_route WHERE installation_id = $1`, installationID)
	fx.Cleanup(t, `DELETE FROM dingtalk_group_presence WHERE installation_id = $1`, installationID)
	fx.Cleanup(t, `DELETE FROM dingtalk_bot_identity WHERE installation_id = $1`, installationID)
	raw, err := json.Marshal(dingtalkRawEvent{AppID: suffix, ConversationTitle: "Release team"})
	if err != nil {
		t.Fatal(err)
	}
	message := channel.InboundMessage{
		EventID: "event-" + suffix, MessageID: "message-" + suffix,
		Type: channel.MsgTypeText, Text: "hello", AddressedToBot: true, Raw: raw,
		Source: channel.Source{ChannelType: TypeDingTalk, ChatType: channel.ChatTypeGroup, ChatID: "cid-" + suffix, SenderID: "staff-member"},
	}
	q := db.New(pool)
	set := NewDingTalkResolverSet(q, pool, nil, nil, nil, nil)
	inst, err := set.Installation.ResolveInstallation(context.Background(), message)
	if err != nil {
		t.Fatal(err)
	}
	return groupRoutingFixture{fx: fx, q: q, set: set, inst: inst, message: message,
		other: util.MustParseUUID(other), finalizer: set.Installation.(*installationResolver)}
}

func TestDingTalkGroupRoutingReassignmentFencesOldWritesDB(t *testing.T) {
	f := newGroupRoutingFixture(t)
	ctx := context.Background()
	initial, err := f.finalizer.FinalizeInstallation(ctx, f.inst, f.message)
	if err != nil || initial.AgentID != f.inst.AgentID || initial.RouteRevision != 1 {
		t.Fatalf("first group discovery: installation=%+v err=%v", initial, err)
	}
	creator := util.MustParseUUID(f.fx.UserID)
	oldSession, err := f.set.Session.EnsureSession(ctx, engine.EnsureSessionParams{Installation: initial, Sender: creator, Message: f.message})
	if err != nil {
		t.Fatal(err)
	}
	routes, err := f.q.ListDingTalkGroupRoutesByWorkspace(ctx, f.inst.WorkspaceID)
	if err != nil || len(routes) != 1 {
		t.Fatalf("group inventory: routes=%v err=%v", routes, err)
	}
	if _, err := f.q.ReassignDingTalkGroupRoute(ctx, db.ReassignDingTalkGroupRouteParams{
		WorkspaceID: f.inst.WorkspaceID, AgentID: f.other, RouteID: routes[0].ID,
	}); err != nil {
		t.Fatal(err)
	}
	_, err = f.set.Session.AppendMessage(ctx, engine.AppendParams{
		InstallationID: initial.ID, Installation: initial, SessionID: oldSession, Sender: creator, Message: f.message,
	})
	if !errors.Is(err, engine.ErrRouteChanged) {
		t.Fatalf("stale append must lose its fence, got %v", err)
	}
	_, err = f.set.Session.StartSession(ctx, engine.StartSessionParams{
		Installation: initial, Creator: creator, Sender: creator, Message: f.message, PersistMessage: true,
	})
	if !errors.Is(err, engine.ErrRouteChanged) {
		t.Fatalf("stale /new must lose its fence, got %v", err)
	}
	_, err = f.set.Session.EnsureSession(ctx, engine.EnsureSessionParams{Installation: initial, Sender: creator, Message: f.message})
	if !errors.Is(err, engine.ErrRouteChanged) {
		t.Fatalf("stale ensure must not recreate the old binding, got %v", err)
	}
	if count := f.fx.Count(t, `SELECT count(*) FROM chat_message WHERE chat_session_id = $1`, oldSession); count != 0 {
		t.Fatalf("rejected old append left %d messages", count)
	}
	if count := f.fx.Count(t, `SELECT count(*) FROM chat_session WHERE workspace_id = $1`, f.fx.WorkspaceID); count != 1 {
		t.Fatalf("rejected route writes left %d sessions, want only the original history", count)
	}
	current, err := f.finalizer.FinalizeInstallation(ctx, initial, f.message)
	if err != nil || current.AgentID != f.other || current.RouteRevision != 2 {
		t.Fatalf("reassignment was overwritten by observation: %+v err=%v", current, err)
	}
	nextSession, err := f.set.Session.EnsureSession(ctx, engine.EnsureSessionParams{Installation: current, Sender: creator, Message: f.message})
	if err != nil || nextSession == oldSession {
		t.Fatalf("new agent must receive a distinct chat: session=%v err=%v", nextSession, err)
	}
	if _, err := f.set.Session.AppendMessage(ctx, engine.AppendParams{
		InstallationID: current.ID, Installation: current, SessionID: nextSession, Sender: creator, Message: f.message,
	}); err != nil {
		t.Fatal(err)
	}
	session, err := f.q.GetChatSession(ctx, nextSession)
	if err != nil || session.AgentID != f.other {
		t.Fatalf("new turn reached wrong agent: session=%+v err=%v", session, err)
	}
	if count := f.fx.Count(t, `SELECT count(*) FROM chat_message WHERE chat_session_id = $1`, nextSession); count != 1 {
		t.Fatalf("new group turn count=%d, want exactly one", count)
	}
	var mentions int64
	f.fx.QueryRow(t, `SELECT mention_count FROM dingtalk_group_presence WHERE installation_id = $1 AND conversation_id = $2`, current.ID, f.message.Source.ChatID).Scan(&mentions)
	if mentions != 1 {
		t.Fatalf("discovery or rejected writes counted as activity: mentions=%d, want one committed message", mentions)
	}
	if _, err := f.q.GetChatSession(ctx, oldSession); err != nil {
		t.Fatalf("reassignment removed chat history: %v", err)
	}
}

func TestDingTalkGroupRoutingReplacesPreMigrationBindingDB(t *testing.T) {
	f := newGroupRoutingFixture(t)
	ctx := context.Background()
	f.fx.Insert(t, "dingtalk_group_route", dbfx.Cols{
		"workspace_id": f.fx.WorkspaceID, "installation_id": f.inst.ID,
		"conversation_id": f.message.Source.ChatID, "agent_id": f.other,
	})
	creator := util.MustParseUUID(f.fx.UserID)
	// Reproduce the pre-migration bug: the existing group route points to the
	// new agent, but the old inbound code still created a default-agent Chat.
	oldSession, err := f.set.Session.EnsureSession(ctx, engine.EnsureSessionParams{Installation: f.inst, Sender: creator, Message: f.message})
	if err != nil {
		t.Fatal(err)
	}
	current, err := f.finalizer.FinalizeInstallation(ctx, f.inst, f.message)
	if err != nil {
		t.Fatal(err)
	}
	nextSession, err := f.set.Session.EnsureSession(ctx, engine.EnsureSessionParams{Installation: current, Sender: creator, Message: f.message})
	if err != nil || nextSession == oldSession {
		t.Fatalf("pre-migration binding survived: session=%v err=%v", nextSession, err)
	}
	if _, err := f.q.GetChannelChatSessionBindingBySession(ctx, db.GetChannelChatSessionBindingBySessionParams{
		ChatSessionID: oldSession, ChannelType: string(TypeDingTalk),
	}); !errors.Is(err, pgx.ErrNoRows) {
		t.Fatalf("old outbound route remains reachable: %v", err)
	}
	if _, err := f.q.GetChatSession(ctx, oldSession); err != nil {
		t.Fatalf("old Chat history must remain: %v", err)
	}
	f.fx.Exec(t, `UPDATE agent SET archived_at = now() WHERE id = $1`, f.other)
	if _, err := f.finalizer.FinalizeInstallation(ctx, current, f.message); !errors.Is(err, engine.ErrTargetAgentArchived) {
		t.Fatalf("archived group target must not fall back to default: %v", err)
	}
	// Direct conversations retain the installation's default agent.
	direct := f.message
	direct.Source.ChatType = channel.ChatTypeP2P
	resolved, err := f.finalizer.FinalizeInstallation(ctx, f.inst, direct)
	if err != nil || resolved.AgentID != f.inst.AgentID || resolved.RouteRevision != 0 {
		t.Fatalf("direct routing changed: %+v err=%v", resolved, err)
	}
}

func TestDingTalkGroupRouteFenceSerializesReassignmentDB(t *testing.T) {
	f := newGroupRoutingFixture(t)
	ctx := context.Background()
	current, err := f.finalizer.FinalizeInstallation(ctx, f.inst, f.message)
	if err != nil {
		t.Fatal(err)
	}
	routes, err := f.q.ListDingTalkGroupRoutesByWorkspace(ctx, f.inst.WorkspaceID)
	if err != nil || len(routes) != 1 {
		t.Fatalf("group routes=%v err=%v", routes, err)
	}
	appendTx, err := f.fx.Pool.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer appendTx.Rollback(ctx)
	if err := groupRouteFence(current, f.message, false)(ctx, appendTx); err != nil {
		t.Fatal(err)
	}
	reassignTx, err := f.fx.Pool.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer reassignTx.Rollback(ctx)
	if _, err := reassignTx.Exec(ctx, `SET LOCAL lock_timeout = '100ms'`); err != nil {
		t.Fatal(err)
	}
	params := db.ReassignDingTalkGroupRouteParams{WorkspaceID: f.inst.WorkspaceID, AgentID: f.other, RouteID: routes[0].ID}
	_, err = db.New(reassignTx).ReassignDingTalkGroupRoute(ctx, params)
	var lockErr *pgconn.PgError
	if !errors.As(err, &lockErr) || lockErr.Code != "55P03" {
		t.Fatalf("reassignment crossed an active message fence: %v", err)
	}
	if err := reassignTx.Rollback(ctx); err != nil {
		t.Fatal(err)
	}
	if err := appendTx.Commit(ctx); err != nil {
		t.Fatal(err)
	}
	if _, err := f.q.ReassignDingTalkGroupRoute(ctx, params); err != nil {
		t.Fatalf("reassignment must proceed after message commits: %v", err)
	}
}
