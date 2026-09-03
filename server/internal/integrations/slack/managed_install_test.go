package slack

import (
	"context"
	"encoding/json"
	"errors"
	"testing"

	"github.com/jackc/pgx/v5/pgtype"
)

// The routing key must separate tenants of the one hosted app: the same app id
// installed into two Slack workspaces yields two installations, and the key
// must never equal a bare BYO app id (which contains no colon).
func TestManagedRoutingKeyIsTenantSpecific(t *testing.T) {
	if got := ManagedRoutingKey("A1", "T1"); got != "A1:T1" {
		t.Fatalf("routing key = %q, want A1:T1", got)
	}
	if ManagedRoutingKey("A1", "T1") == ManagedRoutingKey("A1", "T2") {
		t.Fatal("same app installed into two teams must not share a routing key")
	}
	if ManagedRoutingKey("A1", "T1") == "A1" {
		t.Fatal("managed routing key must not collide with a bare BYO app id")
	}
}

func managedAccess() OAuthAccess {
	return OAuthAccess{
		BotToken:   "xoxb-managed-test",
		AppID:      "A123TEST",
		TeamID:     "T456TEST",
		BotUserID:  "UBOTTEST",
		AuthedUser: "U9TEST",
	}
}

// The persisted row is team-keyed under the nil agent: re-connecting the same
// team updates it in place, and the bot token is sealed (never plaintext).
func TestRegisterManagedPersistsTeamKeyedInstall(t *testing.T) {
	q := &fakeInstallQueries{}
	svc := newTestInstallService(t, q)
	wsID, installerID := testUUID(11), testUUID(12)

	row, err := svc.RegisterManaged(context.Background(), RegisterManagedParams{
		WorkspaceID: wsID,
		InstallerID: installerID,
		Access:      managedAccess(),
	}, nil)
	if err != nil {
		t.Fatalf("register managed: %v", err)
	}
	if !q.byAppIDCalled {
		t.Fatal("managed install must upsert by routing key, not by (workspace, agent)")
	}
	p := q.byAppIDParams
	if p.WorkspaceID != wsID || p.InstallerUserID != installerID {
		t.Errorf("upsert identity = %+v, want workspace/installer from params", p)
	}
	if p.ChannelType != string(TypeSlack) {
		t.Errorf("channel type = %q, want slack", p.ChannelType)
	}
	// Nil agent: the install belongs to the workspace; the team key (not the
	// agent key) arbitrates ownership.
	if p.AgentID != (pgtype.UUID{Valid: true}) {
		t.Errorf("managed agent id = %+v, want the nil UUID", p.AgentID)
	}
	var cfg installConfig
	if err := json.Unmarshal(p.Config, &cfg); err != nil {
		t.Fatalf("stored config is not JSON: %v", err)
	}
	if cfg.AppID != "A123TEST:T456TEST" || cfg.ApiAppID != "A123TEST" || cfg.TeamID != "T456TEST" || cfg.BotUserID != "UBOTTEST" {
		t.Errorf("stored routing identity = %+v, want team-keyed composite", cfg)
	}
	if cfg.Transport != ManagedTransportWebhook {
		t.Errorf("transport = %q, want webhook (no app-level token on a managed install)", cfg.Transport)
	}
	if cfg.AppTokenEncrypted != "" {
		t.Errorf("managed install must not carry an app token, got %q", cfg.AppTokenEncrypted)
	}
	if cfg.BotTokenEncrypted == "" || cfg.BotTokenEncrypted == "xoxb-managed-test" {
		t.Fatal("bot token must be sealed at rest, never plaintext")
	}
	if row.WorkspaceID != wsID || row.Status != "installed" {
		t.Errorf("returned row = %+v, want the installed workspace bot", row)
	}
}

func TestRegisterManagedIncompleteAccessRefused(t *testing.T) {
	q := &fakeInstallQueries{}
	svc := newTestInstallService(t, q)
	base := managedAccess()
	for name, mutate := range map[string]func(*OAuthAccess){
		"empty bot token": func(a *OAuthAccess) { a.BotToken = "" },
		"empty app id":    func(a *OAuthAccess) { a.AppID = "" },
		"empty team id":   func(a *OAuthAccess) { a.TeamID = "" },
	} {
		access := base
		mutate(&access)
		if _, err := svc.RegisterManaged(context.Background(), RegisterManagedParams{
			WorkspaceID: testUUID(11),
			InstallerID: testUUID(12),
			Access:      access,
		}, nil); err == nil {
			t.Errorf("%s: incomplete exchange identity must be refused before touching the DB", name)
		}
	}
	if q.byAppIDCalled {
		t.Error("refused install must not reach the upsert")
	}
}

// A team live-owned by another workspace updates no row (the upsert's atomic
// cross-workspace guard): the caller renders the same conflict the BYO path
// names, so the user disconnects it there first.
func TestRegisterManagedCrossWorkspaceConflict(t *testing.T) {
	q := &fakeInstallQueries{byAppIDNoRows: true}
	svc := newTestInstallService(t, q)
	_, err := svc.RegisterManaged(context.Background(), RegisterManagedParams{
		WorkspaceID: testUUID(11),
		InstallerID: testUUID(12),
		Access:      managedAccess(),
	}, nil)
	if !errors.Is(err, ErrTeamOwnedByAnotherWorkspace) {
		t.Fatalf("cross-workspace team err = %v, want ErrTeamOwnedByAnotherWorkspace", err)
	}
}

// A second team in the same workspace trips the (workspace, nil-agent, slack)
// key: one managed install per workspace, refused with an actionable message.
func TestRegisterManagedSecondTeamConflict(t *testing.T) {
	q := &fakeInstallQueries{byAppIDTaken: true}
	svc := newTestInstallService(t, q)
	_, err := svc.RegisterManaged(context.Background(), RegisterManagedParams{
		WorkspaceID: testUUID(11),
		InstallerID: testUUID(12),
		Access:      managedAccess(),
	}, nil)
	if !errors.Is(err, ErrManagedAlreadyConnected) {
		t.Fatalf("second-team err = %v, want ErrManagedAlreadyConnected", err)
	}
}
