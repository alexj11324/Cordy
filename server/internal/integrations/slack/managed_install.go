package slack

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgtype"

	"github.com/patchbay-ai/patchbay/server/internal/hostedcapacity"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// ManagedTransportWebhook marks a managed (hosted-OAuth) installation: inbound
// arrives on the deployment's Events API webhook, not on a per-installation
// Socket Mode connection, so the config carries no app-level token.
const ManagedTransportWebhook = "webhook"

// ErrManagedAlreadyConnected is returned when a workspace that already holds a
// managed Slack installation for one team tries to connect a DIFFERENT team.
// Both rows would share the nil-agent (workspace, agent, channel_type) key, so
// the second connect is refused with an actionable message instead of stealing
// or duplicating the row.
var ErrManagedAlreadyConnected = errors.New("slack: this workspace already has a managed Slack installation for another Slack workspace — disconnect it first, then connect the new one")

// ManagedRoutingKey is the database routing identity for a managed install:
// one official Slack app installed into many Slack workspaces needs one
// installation per team, keyed in the existing config->>'app_id' slot. BYO
// installs keep the bare app id; the composite can never collide with one
// (a real app id contains no colon).
func ManagedRoutingKey(appID, teamID string) string {
	return appID + ":" + teamID
}

// RegisterManagedParams are the inputs for persisting a hosted-OAuth install:
// the workspace it belongs to, who installed it, and the token + tenant
// identity the callback exchanged the code for.
type RegisterManagedParams struct {
	WorkspaceID pgtype.UUID
	InstallerID pgtype.UUID
	Access      OAuthAccess
}

// RegisterManaged persists a hosted-OAuth installation keyed by team
// (UpsertChannelInstallationByAppID with the ManagedRoutingKey), NOT by agent
// like RegisterBYO: one Slack workspace maps to exactly one installation, and
// re-connecting it — even after a disconnect — updates the row in place. The
// row's agent_id is the nil UUID: managed installs belong to the workspace, and
// the team-keyed upsert (not the agent key) arbitrates ownership, so at most
// one managed install exists per workspace. The bot token is sealed before it
// touches the row, exactly like the BYO path.
//
// Deliberately a single statement with NO dead-owner reclaim (unlike
// persistInstall): the shared reclaim treats any installation whose agent row
// is missing as an orphan, and the nil agent never exists — so it would delete
// a LIVE managed row (including one owned by another workspace) right before
// the upsert, silently transferring the team instead of refusing with
// ErrTeamOwnedByAnotherWorkspace. Same-team reconnect still reactivates the
// revoked row in place via the conflict update, preserving its bindings.
//
// limit is the hosted installation cap resolved by the handler. When set, the
// upsert runs inside a transaction that first takes the workspace capacity
// lock and refuses with hostedcapacity.ErrLimitReached; when nil (self-hosted
// or unlimited), the historical single-statement path runs unchanged.
func (s *InstallService) RegisterManaged(ctx context.Context, p RegisterManagedParams, limit *int64) (db.ChannelInstallation, error) {
	if p.Access.BotToken == "" || p.Access.AppID == "" || p.Access.TeamID == "" {
		return db.ChannelInstallation{}, errors.New("slack: managed OAuth exchange returned an incomplete identity (missing bot token / app id / team id)")
	}
	sealedBot, err := s.box.Seal([]byte(p.Access.BotToken))
	if err != nil {
		return db.ChannelInstallation{}, fmt.Errorf("encrypt managed slack bot token: %w", err)
	}
	var sealedRefresh string
	var expiresAt *time.Time
	if !p.Access.ExpiresAt.IsZero() && p.Access.RefreshToken == "" {
		return db.ChannelInstallation{}, errors.New("slack: expiring credential has no refresh token")
	}
	if p.Access.RefreshToken != "" {
		if p.Access.ExpiresAt.IsZero() {
			return db.ChannelInstallation{}, errors.New("slack: rotating credential has no expiry")
		}
		sealed, err := s.box.Seal([]byte(p.Access.RefreshToken))
		if err != nil {
			return db.ChannelInstallation{}, fmt.Errorf("encrypt managed slack refresh token: %w", err)
		}
		sealedRefresh = base64.StdEncoding.EncodeToString(sealed)
		expires := p.Access.ExpiresAt
		expiresAt = &expires
	}
	routingKey := ManagedRoutingKey(p.Access.AppID, p.Access.TeamID)
	cfgJSON, err := json.Marshal(installConfig{
		AppID:                 routingKey,
		ApiAppID:              p.Access.AppID,
		TeamID:                p.Access.TeamID,
		BotUserID:             p.Access.BotUserID,
		BotTokenEncrypted:     base64.StdEncoding.EncodeToString(sealedBot),
		Transport:             ManagedTransportWebhook,
		RefreshTokenEncrypted: sealedRefresh,
		TokenExpiresAt:        expiresAt,
	})
	if err != nil {
		return db.ChannelInstallation{}, fmt.Errorf("encode managed slack installation config: %w", err)
	}
	// Nil agent: the install belongs to the workspace, not to one agent. The
	// team-keyed conflict target below (not the (workspace, agent, channel)
	// key) is what keeps one team to one row.
	var nilAgent pgtype.UUID
	nilAgent.Valid = true

	upsert := func(q installQueries) (db.ChannelInstallation, error) {
		return q.UpsertChannelInstallationByAppID(ctx, db.UpsertChannelInstallationByAppIDParams{
			WorkspaceID:     p.WorkspaceID,
			AgentID:         nilAgent,
			ChannelType:     string(TypeSlack),
			Config:          cfgJSON,
			InstallerUserID: p.InstallerID,
		})
	}

	if limit == nil {
		inst, err := upsert(s.q)
		if err != nil {
			return db.ChannelInstallation{}, s.classifyManagedUpsertErr(err)
		}
		return inst, nil
	}

	tx, err := s.tx.Begin(ctx)
	if err != nil {
		return db.ChannelInstallation{}, fmt.Errorf("begin managed install tx: %w", err)
	}
	defer func() { _ = tx.Rollback(ctx) }()
	qtx := s.q.WithTx(tx)
	if err := hostedcapacity.AdmitInstall(ctx, qtx, p.WorkspaceID, string(TypeSlack), nilAgent, limit); err != nil {
		return db.ChannelInstallation{}, err
	}
	inst, err := upsert(qtx)
	if err != nil {
		return db.ChannelInstallation{}, s.classifyManagedUpsertErr(err)
	}
	if err := tx.Commit(ctx); err != nil {
		return db.ChannelInstallation{}, fmt.Errorf("commit managed slack install: %w", err)
	}
	return inst, nil
}

// classifyManagedUpsertErr turns the upsert's raw error into the accurate
// conflict sentinel the handler renders.
func (s *InstallService) classifyManagedUpsertErr(err error) error {
	if errors.Is(err, pgx.ErrNoRows) {
		// The conflict update fenced on the same workspace touched no row:
		// the team is live-owned by a DIFFERENT Patchbay workspace (the
		// query's atomic cross-workspace guard).
		return ErrTeamOwnedByAnotherWorkspace
	}
	var pgErr *pgconn.PgError
	if errors.As(err, &pgErr) && pgErr.Code == pgUniqueViolation {
		// The (workspace, nil-agent, slack) key is taken by a managed row
		// for a DIFFERENT team: one managed install per workspace.
		return ErrManagedAlreadyConnected
	}
	return fmt.Errorf("upsert managed slack installation: %w", err)
}
