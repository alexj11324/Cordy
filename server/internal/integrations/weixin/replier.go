package weixin

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"net/url"
	"strings"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel/engine"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

const (
	weixinAgentOfflineText  = "⚠️ The agent is offline. Your message was saved and will continue when its runtime reconnects."
	weixinAgentArchivedText = "⚠️ This agent has been archived. Please contact a workspace admin."
	weixinFreshPendingText  = "✅ Fresh start ready. Your next message will run without prior context."
	weixinIssueUsageText    = "Please include a task title:\n\n/issue <title>\n[description] (optional)"
)

type OutboundReplier struct {
	binding     *BindingTokenService
	decrypt     Decrypter
	appURL      string
	bindingPath string
	logger      *slog.Logger
}

type OutboundReplierConfig struct {
	Binding     *BindingTokenService
	Decrypt     Decrypter
	AppURL      string
	BindingPath string
	Logger      *slog.Logger
}

var _ engine.OutboundReplier = (*OutboundReplier)(nil)

func NewOutboundReplier(cfg OutboundReplierConfig) *OutboundReplier {
	logger := cfg.Logger
	if logger == nil {
		logger = slog.Default()
	}
	path := cfg.BindingPath
	if path == "" {
		path = "/weixin/bind"
	}
	if !strings.HasPrefix(path, "/") {
		path = "/" + path
	}
	return &OutboundReplier{
		binding: cfg.Binding, decrypt: cfg.Decrypt,
		appURL: strings.TrimRight(strings.TrimSpace(cfg.AppURL), "/"), bindingPath: path, logger: logger,
	}
}

func (r *OutboundReplier) Reply(ctx context.Context, inst engine.ResolvedInstallation, msg channel.InboundMessage, result engine.Result) {
	var text string
	switch result.Outcome {
	case engine.OutcomeNeedsBinding:
		if err := r.sendBindingPrompt(ctx, inst, msg, result); err != nil {
			r.logger.WarnContext(ctx, "weixin replier: binding prompt failed", "installation_id", util.UUIDToString(inst.ID), "error", err)
		}
		return
	case engine.OutcomeAgentOffline:
		text = weixinAgentOfflineText
	case engine.OutcomeAgentArchived:
		text = weixinAgentArchivedText
	case engine.OutcomeFreshPending:
		text = weixinFreshPendingText
	case engine.OutcomeIssueUsage:
		text = weixinIssueUsageText
	case engine.OutcomeIngested:
		if result.IssueID.Valid {
			text = weixinIssueCreatedText(result)
		}
	default:
		return
	}
	if err := r.post(ctx, inst, msg, text); err != nil {
		r.logger.WarnContext(ctx, "weixin replier: outcome reply failed", "installation_id", util.UUIDToString(inst.ID), "error", err)
	}
}

func (r *OutboundReplier) sendBindingPrompt(ctx context.Context, inst engine.ResolvedInstallation, msg channel.InboundMessage, result engine.Result) error {
	if r.binding == nil || r.appURL == "" {
		return errors.New("weixin: binding service or app URL is not configured")
	}
	sender := result.Sender
	if sender == "" {
		sender = msg.Source.SenderID
	}
	token, err := r.binding.Mint(ctx, inst.WorkspaceID, inst.ID, sender)
	if err != nil {
		return err
	}
	text := fmt.Sprintf("👋 Link your Patchbay account to continue:\n%s%s?token=%s\n(This link expires in 15 minutes.)",
		r.appURL, r.bindingPath, url.QueryEscape(token.Raw))
	return r.post(ctx, inst, msg, text)
}

func (r *OutboundReplier) post(ctx context.Context, inst engine.ResolvedInstallation, msg channel.InboundMessage, text string) error {
	row, ok := inst.Platform.(db.ChannelInstallation)
	if !ok {
		return errors.New("weixin: installation row unavailable")
	}
	credentials, err := DecodeCredentials(row.Config, r.decrypt)
	if err != nil {
		return err
	}
	baseURL, err := ValidateProviderBaseURL(credentials.BaseURL)
	if err != nil {
		return err
	}
	raw, err := decodeRawEvent(msg)
	if err != nil {
		return err
	}
	if _, err := NewClient(baseURL, credentials.BotToken, nil).SendText(ctx, msg.Source.SenderID, raw.ContextToken, text); err != nil {
		return fmt.Errorf("weixin: send outcome reply: %w", err)
	}
	return nil
}

func weixinIssueCreatedText(result engine.Result) string {
	identifier := strings.TrimSpace(result.IssueIdentifier)
	if identifier == "" {
		identifier = util.UUIDToString(result.IssueID)
	}
	title := strings.TrimSpace(result.IssueTitle)
	if title == "" {
		return "✅ Created " + identifier
	}
	return "✅ Created " + identifier + " — " + title
}
