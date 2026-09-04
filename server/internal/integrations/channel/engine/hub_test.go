package engine

import (
	"context"
	"encoding/json"
	"errors"
	"strings"
	"testing"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

type fakeHubQueries struct {
	agents  []db.Agent
	targets []db.AgentInvocationTarget
	route   []byte
	locale  string
	err     error
	lookup  db.GetChannelHubRouteParams
}

func (q *fakeHubQueries) ListAgents(context.Context, pgtype.UUID) ([]db.Agent, error) {
	return q.agents, q.err
}
func (q *fakeHubQueries) ListAgentInvocationTargetsByAgentIDs(context.Context, []pgtype.UUID) ([]db.AgentInvocationTarget, error) {
	return q.targets, q.err
}
func (q *fakeHubQueries) GetChannelHubRoute(_ context.Context, p db.GetChannelHubRouteParams) ([]byte, error) {
	q.lookup = p
	if q.route == nil {
		return nil, pgx.ErrNoRows
	}
	return q.route, q.err
}
func (q *fakeHubQueries) GetUser(context.Context, pgtype.UUID) (db.User, error) {
	return db.User{Language: pgtype.Text{String: q.locale, Valid: q.locale != ""}}, q.err
}

func hubTestAgent(id byte, name string, owner pgtype.UUID, permission string) db.Agent {
	return db.Agent{ID: uid(id), Name: name, OwnerID: owner, WorkspaceID: uid(10), Kind: "user", PermissionMode: permission}
}

func hubTestRoute(t *testing.T, id pgtype.UUID) []byte {
	t.Helper()
	raw, err := json.Marshal(map[string]string{"hub_agent_id": util.UUIDToString(id)})
	if err != nil {
		t.Fatal(err)
	}
	return raw
}

func TestHubListsOnlyInvocableAgentsAndRetainsProductAgents(t *testing.T) {
	user, other := uid(11), uid(12)
	q := &fakeHubQueries{agents: []db.Agent{
		hubTestAgent(1, "Patrick", user, "private"),
		hubTestAgent(2, "Other private", other, "private"),
		hubTestAgent(3, "Workspace", other, "public_to"),
		hubTestAgent(4, "Member", other, "public_to"),
		hubTestAgent(5, "Different member", other, "public_to"),
		hubTestAgent(6, "Team only", other, "public_to"),
		hubTestAgent(7, "Hidden carrier", user, "private"),
		hubTestAgent(8, "Archived", user, "private"),
		hubTestAgent(9, "Foreign workspace", user, "private"),
	}, targets: []db.AgentInvocationTarget{
		{AgentID: uid(3), TargetType: "workspace"},
		{AgentID: uid(4), TargetType: "member", TargetID: user},
		{AgentID: uid(5), TargetType: "member", TargetID: other},
		{AgentID: uid(6), TargetType: "team", TargetID: user},
	}}
	q.agents[6].Kind = "system"
	q.agents[7].ArchivedAt.Valid = true
	q.agents[8].WorkspaceID = uid(99)
	h := &PostgresHubRouter{q: q}
	agents, err := h.availableAgents(context.Background(), uid(10), user)
	if err != nil {
		t.Fatal(err)
	}
	if len(agents) != 3 || agents[0].ID != uid(1) || agents[1].ID != uid(3) || agents[2].ID != uid(4) {
		t.Fatal("Hub selection broadened invocation rights or hid a product-defined user Agent")
	}
}

func TestHubSelectionCommandsAndStoredChoice(t *testing.T) {
	user := uid(11)
	q := &fakeHubQueries{agents: []db.Agent{hubTestAgent(1, "Writer", user, "private"), hubTestAgent(2, "Reviewer", user, "private")}}
	h := &PostgresHubRouter{q: q}
	inst := ResolvedInstallation{ID: uid(20), WorkspaceID: uid(10)}
	for _, tc := range []struct {
		text    string
		agent   pgtype.UUID
		handled bool
		ensure  bool
	}{
		{"hello", uid(1), false, false},
		{"/agents", uid(1), true, false},
		{"/agents 2", uid(2), true, true},
		{"/agent@patchbay reviewer", uid(2), true, true},
		{"/agents " + util.UUIDToString(uid(2)), uid(2), true, true},
		{"/agents 0", uid(1), true, false},
		{"/agents missing", uid(1), true, false},
		{"/issue fix login", uid(1), false, false},
	} {
		t.Run(tc.text, func(t *testing.T) {
			got, err := h.Resolve(context.Background(), inst, ResolvedIdentity{UserID: user}, channel.InboundMessage{CommandText: tc.text}, "C1:thread-1")
			if err != nil || got.AgentID != tc.agent || got.Handled != tc.handled || got.EnsureSession != tc.ensure {
				t.Fatalf("Hub command contract: result=%+v error=%v", got, err)
			}
			if tc.handled && got.ReplyText == "" {
				t.Fatal("control command has no user-visible reply")
			}
		})
	}
	q.route = hubTestRoute(t, uid(2))
	got, err := h.Resolve(context.Background(), inst, ResolvedIdentity{UserID: user}, channel.InboundMessage{CommandText: "continue"}, "C1:thread-1")
	if err != nil || got.AgentID != uid(2) || got.Handled || got.SwitchesAgent {
		t.Fatal("ordinary message did not preserve the stored Agent without interrupting debounce")
	}
	if q.lookup.ChannelID != "C1" || q.lookup.BindingKey != "C1:thread-1" {
		t.Fatal("Hub lookup lost the exact conversation or channel fallback")
	}
	q.agents = q.agents[:1]
	got, err = h.Resolve(context.Background(), inst, ResolvedIdentity{UserID: user}, channel.InboundMessage{}, "C1:thread-1")
	if err != nil || got.AgentID != uid(1) || !got.SwitchesAgent {
		t.Fatal("revoked choice must fall back to an invocable Agent and fence the old pending turn")
	}
}

func TestHubRepliesUseMemberLocaleAndEscapeUntrustedNames(t *testing.T) {
	q := &fakeHubQueries{locale: "zh-Hans"}
	h := &PostgresHubRouter{q: q}
	inst, identity := ResolvedInstallation{WorkspaceID: uid(10)}, ResolvedIdentity{UserID: uid(11)}
	res, err := h.Resolve(context.Background(), inst, identity, channel.InboundMessage{}, "D1")
	if err != nil || !res.Handled || !strings.Contains(res.ReplyText, "工作区") || res.AgentID.Valid {
		t.Fatal("empty Hub did not return the member's localized no-Agent notice")
	}
	q.agents = []db.Agent{hubTestAgent(1, "<@U1>\n[click](https://example.test)", identity.UserID, "private")}
	res, err = h.Resolve(context.Background(), inst, identity, channel.InboundMessage{CommandText: "/agents"}, "D1")
	if err != nil || !strings.Contains(res.ReplyText, "可用智能体") || strings.Contains(res.ReplyText, "<@U1>") || strings.Contains(res.ReplyText, "](") {
		t.Fatal("Hub menu lost localization or introduced provider markup from an Agent name")
	}
	q.err = errors.New("database unavailable")
	if _, err := h.Resolve(context.Background(), inst, identity, channel.InboundMessage{}, "D1"); err == nil {
		t.Fatal("a failed Agent lookup must not silently pick or acknowledge an Agent")
	}
}
