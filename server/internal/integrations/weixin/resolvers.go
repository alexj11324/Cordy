package weixin

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"strings"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel/engine"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
)

type ContextSealer func([]byte) ([]byte, error)

type weixinBindingConfig struct {
	UserID                string `json:"user_id"`
	ContextTokenEncrypted string `json:"context_token_encrypted"`
}

// NewResolverSet wires iLink into the unchanged channel engine. The adapter
// owns only provider-specific routing and context-token storage; session,
// dedup, membership, issue and task behavior remain shared.
func NewResolverSet(q *db.Queries, tx engine.TxStarter, pool *pgxpool.Pool, replier engine.OutboundReplier, seal ContextSealer) engine.ResolverSet {
	set := engine.ResolverSet{
		Installation: &installationResolver{q: q},
		Identity:     &identityResolver{q: q},
		Dedup:        &deduper{q: q},
		Session: &sessionBinder{
			session: engine.NewChatSession(q, tx, TypeWeixin, engine.SessionTitles{
				Group: "Weixin chat", Direct: "Weixin direct message", Fallback: "Weixin chat",
			}),
			pool: pool,
			seal: seal,
		},
		Audit:      &auditor{q: q},
		Replier:    replier,
		OriginType: OriginWeixinChat,
	}
	return set
}

var (
	_ engine.InstallationResolver = (*installationResolver)(nil)
	_ engine.IdentityResolver     = (*identityResolver)(nil)
	_ engine.Deduper              = (*deduper)(nil)
	_ engine.SessionBinder        = (*sessionBinder)(nil)
	_ engine.Auditor              = (*auditor)(nil)
)

func decodeRawEvent(msg channel.InboundMessage) (RawEvent, error) {
	if len(msg.Raw) == 0 {
		return RawEvent{}, errors.New("weixin: inbound Raw is empty")
	}
	var raw RawEvent
	if err := json.Unmarshal(msg.Raw, &raw); err != nil {
		return RawEvent{}, fmt.Errorf("decode weixin inbound Raw: %w", err)
	}
	if strings.TrimSpace(raw.BotID) == "" || strings.TrimSpace(raw.ContextToken) == "" {
		return RawEvent{}, errors.New("weixin: inbound Raw is missing bot or context identity")
	}
	return raw, nil
}

func validDirectSource(msg channel.InboundMessage) bool {
	return msg.Source.ChannelType == TypeWeixin && msg.Source.ChatType == channel.ChatTypeP2P &&
		strings.TrimSpace(msg.Source.ChatID) != "" &&
		strings.TrimSpace(msg.Source.ChatID) == strings.TrimSpace(msg.Source.SenderID)
}

type installationResolver struct{ q *db.Queries }

func (r *installationResolver) ResolveInstallation(ctx context.Context, msg channel.InboundMessage) (engine.ResolvedInstallation, error) {
	if !validDirectSource(msg) {
		return engine.ResolvedInstallation{}, engine.ErrInstallationNotFound
	}
	raw, err := decodeRawEvent(msg)
	if err != nil {
		return engine.ResolvedInstallation{}, err
	}
	if r.q == nil {
		return engine.ResolvedInstallation{}, errors.New("weixin: installation query is not configured")
	}
	inst, err := r.q.GetChannelInstallationByAppID(ctx, db.GetChannelInstallationByAppIDParams{
		ChannelType: string(TypeWeixin), AppID: raw.BotID,
	})
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return engine.ResolvedInstallation{}, engine.ErrInstallationNotFound
		}
		return engine.ResolvedInstallation{}, err
	}
	if inst.ChannelType != string(TypeWeixin) || inst.WorkspaceID.Valid == false || inst.AgentID.Valid == false {
		return engine.ResolvedInstallation{}, engine.ErrInstallationNotFound
	}
	return engine.ResolvedInstallation{
		ID: inst.ID, WorkspaceID: inst.WorkspaceID, AgentID: inst.AgentID,
		InstallerUserID: inst.InstallerUserID, Installed: inst.Status == "installed", Platform: inst,
	}, nil
}

type identityResolver struct{ q *db.Queries }

func (r *identityResolver) ResolveSender(ctx context.Context, inst engine.ResolvedInstallation, msg channel.InboundMessage) (engine.ResolvedIdentity, error) {
	if r.q == nil || !inst.ID.Valid || !inst.WorkspaceID.Valid || !validDirectSource(msg) {
		return engine.ResolvedIdentity{}, engine.ErrSenderUnbound
	}
	binding, err := r.q.GetChannelUserBindingByUserID(ctx, db.GetChannelUserBindingByUserIDParams{
		InstallationID: inst.ID, ChannelUserID: strings.TrimSpace(msg.Source.SenderID),
	})
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return engine.ResolvedIdentity{}, engine.ErrSenderUnbound
		}
		return engine.ResolvedIdentity{}, err
	}
	// The generic binding query is installation-scoped, but these fields are
	// still checked explicitly because the schema has no FK by policy.
	if binding.ChannelType != string(TypeWeixin) || binding.InstallationID != inst.ID || binding.WorkspaceID != inst.WorkspaceID ||
		binding.ChannelUserID != strings.TrimSpace(msg.Source.SenderID) {
		return engine.ResolvedIdentity{}, engine.ErrSenderUnbound
	}
	if _, err := r.q.GetMemberByUserAndWorkspace(ctx, db.GetMemberByUserAndWorkspaceParams{
		UserID: binding.PatchbayUserID, WorkspaceID: inst.WorkspaceID,
	}); err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return engine.ResolvedIdentity{}, engine.ErrSenderNotMember
		}
		return engine.ResolvedIdentity{}, err
	}
	return engine.ResolvedIdentity{UserID: binding.PatchbayUserID}, nil
}

type deduper struct{ q *db.Queries }

func (d *deduper) Claim(ctx context.Context, installationID pgtype.UUID, messageID string) (pgtype.UUID, error) {
	row, err := d.q.ClaimChannelInboundDedup(ctx, db.ClaimChannelInboundDedupParams{
		InstallationID: installationID, MessageID: messageID,
	})
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return pgtype.UUID{}, engine.ErrDuplicate
		}
		return pgtype.UUID{}, err
	}
	return row.ClaimToken, nil
}

func (d *deduper) Mark(ctx context.Context, installationID pgtype.UUID, messageID string, claimToken pgtype.UUID) error {
	_, err := d.q.MarkChannelInboundDedupProcessed(ctx, db.MarkChannelInboundDedupProcessedParams{
		InstallationID: installationID, MessageID: messageID, ClaimToken: claimToken,
	})
	return err
}

func (d *deduper) Release(ctx context.Context, installationID pgtype.UUID, messageID string, claimToken pgtype.UUID) error {
	_, err := d.q.ReleaseChannelInboundDedup(ctx, db.ReleaseChannelInboundDedupParams{
		InstallationID: installationID, MessageID: messageID, ClaimToken: claimToken,
	})
	return err
}

type chatSession interface {
	EnsureSession(context.Context, engine.EnsureSessionInput) (pgtype.UUID, error)
	StartSession(context.Context, engine.StartSessionInput) (engine.StartSessionResult, error)
	MarkPendingFresh(context.Context, pgtype.UUID, string) error
	AppendUserMessage(context.Context, engine.AppendInput) (engine.AppendResult, error)
	BindMediaRefs(context.Context, engine.BindMediaInput) error
}

type sessionBinder struct {
	session chatSession
	pool    *pgxpool.Pool
	seal    ContextSealer
}

func (s *sessionBinder) bindingFor(msg channel.InboundMessage) ([]byte, error) {
	if !validDirectSource(msg) {
		return nil, errors.New("weixin: inbound source is not a fenced direct chat")
	}
	raw, err := decodeRawEvent(msg)
	if err != nil {
		return nil, err
	}
	if s.seal == nil {
		return nil, errors.New("weixin: context token sealer is not configured")
	}
	sealed, err := s.seal([]byte(raw.ContextToken))
	if err != nil {
		return nil, fmt.Errorf("seal weixin context token: %w", err)
	}
	return json.Marshal(weixinBindingConfig{
		UserID:                msg.Source.SenderID,
		ContextTokenEncrypted: base64.StdEncoding.EncodeToString(sealed),
	})
}

func (s *sessionBinder) EnsureSession(ctx context.Context, p engine.EnsureSessionParams) (pgtype.UUID, error) {
	config, err := s.bindingFor(p.Message)
	if err != nil {
		return pgtype.UUID{}, err
	}
	if !validDirectSource(p.Message) {
		return pgtype.UUID{}, errors.New("weixin: only direct chat sessions are supported")
	}
	id, err := s.session.EnsureSession(ctx, engine.EnsureSessionInput{
		WorkspaceID: p.Installation.WorkspaceID, AgentID: p.Installation.AgentID,
		InstallationID: p.Installation.ID, Sender: p.Sender,
		BindingKey: p.Message.Source.ChatID, BindingConfig: config,
		ChatType: p.Message.Source.ChatType,
	})
	if err != nil {
		return pgtype.UUID{}, err
	}
	if s.pool == nil {
		return pgtype.UUID{}, errors.New("weixin: binding config store is not configured")
	}
	if err := mergeBindingConfig(ctx, s.pool, p.Installation.ID, p.Message.Source.ChatID, config); err != nil {
		return pgtype.UUID{}, err
	}
	return id, nil
}

func (s *sessionBinder) StartSession(ctx context.Context, p engine.StartSessionParams) (engine.StartSessionResult, error) {
	config, err := s.bindingFor(p.Message)
	if err != nil {
		return engine.StartSessionResult{}, err
	}
	if !validDirectSource(p.Message) {
		return engine.StartSessionResult{}, errors.New("weixin: only direct chat sessions are supported")
	}
	result, err := s.session.StartSession(ctx, engine.StartSessionInput{
		EnsureSessionInput: engine.EnsureSessionInput{
			WorkspaceID: p.Installation.WorkspaceID, AgentID: p.Installation.AgentID,
			InstallationID: p.Installation.ID, Sender: p.Creator,
			BindingKey: p.Message.Source.ChatID, BindingConfig: config,
			ChatType: p.Message.Source.ChatType,
		},
		Initiator: p.Sender, Body: p.Message.Text, MessageID: p.Message.MessageID,
		ClaimToken: p.ClaimToken, MediaPendingSeconds: p.MediaPendingSeconds,
		PersistMessage: p.PersistMessage, HistoryBoundaryPending: p.HistoryBoundaryPending,
		BeforeCommit: p.BeforeCommit,
	})
	if err != nil {
		return engine.StartSessionResult{}, err
	}
	return result, nil
}

func (s *sessionBinder) MarkPendingFresh(ctx context.Context, sessionID pgtype.UUID, messageID string) error {
	return s.session.MarkPendingFresh(ctx, sessionID, messageID)
}

func (s *sessionBinder) AppendMessage(ctx context.Context, p engine.AppendParams) (engine.AppendResult, error) {
	if !validDirectSource(p.Message) {
		return engine.AppendResult{}, errors.New("weixin: inbound source is not a fenced direct chat")
	}
	commandText := p.Message.CommandText
	if commandText == "" {
		commandText = p.Message.Text
	}
	return s.session.AppendUserMessage(ctx, engine.AppendInput{
		SessionID: p.SessionID, Sender: p.Sender, InstallationID: p.InstallationID,
		Body: p.Message.Text, CommandText: commandText, MessageID: p.Message.MessageID,
		ClaimToken: p.ClaimToken, MediaPendingSeconds: p.MediaPendingSeconds,
		ForceFresh: p.Message.ForceFresh,
	})
}

func (s *sessionBinder) BindMedia(ctx context.Context, p engine.BindMediaParams) (engine.BindMediaResult, error) {
	return engine.BindMediaResult{}, s.session.BindMediaRefs(ctx, engine.BindMediaInput{
		MessageID: p.MessageID, SessionID: p.SessionID, WorkspaceID: p.WorkspaceID,
		Sender: p.Sender, IssueID: p.IssueID, IssueDescriptionBase: p.IssueDescriptionBase,
		IssueCommandText: p.IssueCommandText, Body: p.Body, MediaRefs: p.MediaRefs,
	})
}

type auditor struct{ q *db.Queries }

func (a *auditor) RecordDrop(ctx context.Context, installationID pgtype.UUID, msg channel.InboundMessage, reason engine.DropReason) error {
	raw, _ := decodeRawEvent(msg)
	if a.q == nil {
		return errors.New("weixin: audit query is not configured")
	}
	return a.q.RecordChannelInboundDrop(ctx, db.RecordChannelInboundDropParams{
		ID: dbid.NewV7(), ChannelType: string(TypeWeixin), EventType: raw.EventType,
		DropReason: string(reason), InstallationID: installationID,
		ChannelChatID: nullableText(msg.Source.ChatID), ChannelEventID: nullableText(msg.EventID),
		ChannelMessageID: nullableText(msg.MessageID),
	})
}

func nullableText(value string) pgtype.Text {
	if value == "" {
		return pgtype.Text{}
	}
	return pgtype.Text{String: value, Valid: true}
}
