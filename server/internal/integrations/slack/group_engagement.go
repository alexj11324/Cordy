package slack

import (
	"context"
	"errors"

	"github.com/jackc/pgx/v5"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel/engine"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// This file implements buzz-style group continuation for Slack: once the bot
// has been addressed inside a thread, follow-up messages in that same thread
// are treated as addressed without requiring a fresh @-mention every turn.
// Engagement is the live (non-retired) channel_chat_session_binding for the
// thread root — session state owned by the shared session service, never
// per-connection adapter memory. That is the exact property the v1 addressing
// policy in inbound.go deferred to this layer.

// engagementQueries is the narrow slice of generated queries the engagement
// check needs. *db.Queries satisfies it; tests supply a fake.
type engagementQueries interface {
	GetChannelChatSessionBinding(ctx context.Context, arg db.GetChannelChatSessionBindingParams) (db.ChannelChatSessionBinding, error)
}

type engagementChecker struct{ q engagementQueries }

// EngagedInThread reports whether inst already owns a live session binding for
// msg's thread, i.e. the bot joined this thread on an earlier addressed turn.
// Only thread replies can continue: DMs address every message already, and a
// top-level channel message starts a new thread root (its binding cannot
// pre-exist), so both return false without touching the store. A missing row
// (pgx.ErrNoRows) means "never engaged"; any other store error is returned so
// the Router can release the dedup claim and let a redelivery retry.
func (c *engagementChecker) EngagedInThread(ctx context.Context, inst engine.ResolvedInstallation, msg channel.InboundMessage) (bool, error) {
	if msg.Source.ChatType != channel.ChatTypeGroup {
		return false, nil
	}
	// A top-level message is its own thread root: continuation is impossible
	// because the binding key (channel:root) is minted from this message's own
	// ts and no earlier turn could have created it.
	if msg.Source.ThreadID == "" || msg.Source.ThreadID == msg.MessageID {
		return false, nil
	}
	bindingKey, _, _ := slackSessionRouting(msg)
	_, err := c.q.GetChannelChatSessionBinding(ctx, db.GetChannelChatSessionBindingParams{
		InstallationID: inst.ID,
		ChannelChatID:  bindingKey,
	})
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return false, nil
		}
		return false, err
	}
	return true, nil
}
