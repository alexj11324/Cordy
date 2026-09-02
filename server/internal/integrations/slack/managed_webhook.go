package slack

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/slack-go/slack"
	"github.com/slack-go/slack/slackevents"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel/engine"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// This file is the managed (hosted-OAuth) Events API ingress. BYO installs
// receive events over their own Socket Mode connection (slack_channel.go);
// managed installs carry no app-level token, so Slack delivers to the
// deployment's public webhook instead, signed with the app's signing secret.
// The endpoint verifies the signature, ACKs inside Slack's 3-second window,
// and hands normalized messages to the same engine pipeline — routing,
// identity, dedup, engagement, and session are shared, only the transport
// differs.

// ManagedEventsPath is the public webhook Slack calls per event. It is public
// (no Patchbay auth): authenticity comes from the HMAC-SHA256 request
// signature, and tenant routing comes from the event's api_app_id + team_id.
const ManagedEventsPath = "/api/integrations/slack/events"

// ManagedSlashPath is the public webhook Slack calls per slash-command
// invocation (the Socket Mode connection that carries them for BYO installs
// does not exist for managed installs). Same authenticity story as events;
// replay protection comes from the trigger_id dedup claim the slash processor
// takes before enqueueing, since there is no Socket Mode envelope id here.
const ManagedSlashPath = "/api/integrations/slack/commands"

// managedWebhookBodyLimit caps one Events API delivery. Slack event bodies are
// kilobytes; anything larger is not a real delivery.
const managedWebhookBodyLimit = 1 << 20

// managedWebhookTimeout bounds the detached engine dispatch of one delivery.
// The HTTP response itself is immediate (Slack only needs the ACK); this keeps
// a wedged pipeline from piling up goroutines behind a flood of retries.
const managedWebhookTimeout = 30 * time.Second

// appIDLookupQueries is the narrow slice of generated queries tenant routing
// needs. *db.Queries satisfies it; tests supply a fake.
type appIDLookupQueries interface {
	GetChannelInstallationByAppID(ctx context.Context, arg db.GetChannelInstallationByAppIDParams) (db.ChannelInstallation, error)
}

// lookupInstallation maps an event's (api_app_id, team_id) to its installation.
// BYO rows store the bare app id in the routing slot, so the direct lookup
// stays the hot path with identical behavior; managed rows store the tenant
// composite (ManagedRoutingKey), which a bare app id can never collide with
// (real app ids contain no colon), so the composite is tried only on a miss.
// Either way the event's team must match the installed team's, or the event
// belongs to a foreign workspace sharing the app and is refused.
func lookupInstallation(ctx context.Context, q appIDLookupQueries, apiAppID, teamID string) (db.ChannelInstallation, error) {
	inst, err := q.GetChannelInstallationByAppID(ctx, db.GetChannelInstallationByAppIDParams{
		ChannelType: string(TypeSlack),
		AppID:       apiAppID,
	})
	if err == nil {
		if !installationServesTeam(inst.Config, teamID) {
			return db.ChannelInstallation{}, engine.ErrInstallationNotFound
		}
		return inst, nil
	}
	if !errors.Is(err, pgx.ErrNoRows) || teamID == "" {
		return db.ChannelInstallation{}, err
	}
	managed, merr := q.GetChannelInstallationByAppID(ctx, db.GetChannelInstallationByAppIDParams{
		ChannelType: string(TypeSlack),
		AppID:       ManagedRoutingKey(apiAppID, teamID),
	})
	if merr != nil {
		return db.ChannelInstallation{}, merr
	}
	if !installationServesTeam(managed.Config, teamID) {
		return db.ChannelInstallation{}, engine.ErrInstallationNotFound
	}
	return managed, nil
}

// slashEnqueuer is the narrow slice of SlashCommandProcessor the webhook
// needs. The concrete processor satisfies it; tests supply a stub.
type slashEnqueuer interface {
	HandleEnvelope(ctx context.Context, cmd slack.SlashCommand, envelopeID string)
}

// ManagedWebhookConfig configures the Events API ingress. Queries routes the
// tenant, Handle is the engine entry point (Router.Handle), and SigningSecret
// is the Slack app's signing secret — empty disables the endpoint with 503 so
// a deployment that never configured it fails loudly instead of accepting
// unsigned deliveries. Slash wires /issue-/new-/clear-over-webhook; nil
// leaves slash handling off (events still flow).
type ManagedWebhookConfig struct {
	Queries       appIDLookupQueries
	Handle        channel.InboundHandler
	Slash         slashEnqueuer
	SigningSecret string
	Logger        *slog.Logger
}

// ManagedWebhook serves the deployment-wide Slack Events API webhook.
type ManagedWebhook struct {
	q      appIDLookupQueries
	handle channel.InboundHandler
	slash  slashEnqueuer
	secret string
	logger *slog.Logger
}

// NewManagedWebhook builds the ingress. Handle may be nil in tests that only
// exercise verification and parsing; deliveries then ACK without dispatch.
func NewManagedWebhook(cfg ManagedWebhookConfig) (*ManagedWebhook, error) {
	if cfg.Queries == nil {
		return nil, errors.New("slack: ManagedWebhook requires queries")
	}
	logger := cfg.Logger
	if logger == nil {
		logger = slog.Default()
	}
	return &ManagedWebhook{q: cfg.Queries, handle: cfg.Handle, slash: cfg.Slash, secret: cfg.SigningSecret, logger: logger}, nil
}

// verifiedBody reads the bounded body and checks the HMAC-SHA256 request
// signature shared by both webhook entry points. It returns the raw body for
// downstream parsing.
func (w *ManagedWebhook) verifiedBody(r *http.Request) ([]byte, error) {
	if w.secret == "" {
		return nil, errWebhookUnconfigured
	}
	body, err := io.ReadAll(io.LimitReader(r.Body, managedWebhookBodyLimit+1))
	if err != nil {
		return nil, errBadBody
	}
	if int64(len(body)) > managedWebhookBodyLimit {
		return nil, errBodyTooLarge
	}
	verifier, err := slack.NewSecretsVerifier(r.Header, w.secret)
	if err != nil {
		return nil, errBadSignature
	}
	if _, err := verifier.Write(body); err != nil {
		return nil, errBadSignature
	}
	if err := verifier.Ensure(); err != nil {
		return nil, errBadSignature
	}
	return body, nil
}

var (
	errWebhookUnconfigured = errors.New("slack managed webhook is not configured")
	errBadBody             = errors.New("cannot read body")
	errBodyTooLarge        = errors.New("body too large")
	errBadSignature        = errors.New("invalid signature")
)

// HandleEvents serves POST ManagedEventsPath: verify, ACK fast, dispatch
// detached. Verification failures are 401; malformed bodies are 400; anything
// addressed to no installation (or to a revoked one — the engine decides that)
// is ACKed and dropped by the shared pipeline, never retried by Slack.
func (w *ManagedWebhook) HandleEvents(rw http.ResponseWriter, r *http.Request) {
	body, err := w.verifiedBody(r)
	if err != nil {
		writeWebhookError(rw, err)
		return
	}
	event, err := slackevents.ParseEvent(body, slackevents.OptionNoVerifyToken())
	if err != nil {
		http.Error(rw, "cannot parse event", http.StatusBadRequest)
		return
	}
	switch event.Type {
	case slackevents.URLVerification:
		var challenge slackevents.ChallengeResponse
		if err := json.Unmarshal(body, &challenge); err != nil {
			http.Error(rw, "cannot parse challenge", http.StatusBadRequest)
			return
		}
		rw.Header().Set("Content-Type", "text/plain")
		_, _ = rw.Write([]byte(challenge.Challenge))
		return
	case slackevents.CallbackEvent:
		// ACK first: Slack retries on anything but 2xx inside ~3s, and the
		// engine pipeline (identity, session, enqueue) is far slower.
		rw.WriteHeader(http.StatusOK)
		w.dispatchDetached(event)
		return
	default:
		rw.WriteHeader(http.StatusOK)
		return
	}
}

// writeWebhookError maps webhook verification failures to status codes. The
// unconfigured case is 503 (fix the deployment), bad bodies are 400, and
// signature problems are 401 — never 200, so Slack keeps retrying nothing and
// the operator sees the misconfiguration in the response.
func writeWebhookError(rw http.ResponseWriter, err error) {
	switch {
	case errors.Is(err, errWebhookUnconfigured):
		http.Error(rw, err.Error(), http.StatusServiceUnavailable)
	case errors.Is(err, errBodyTooLarge):
		http.Error(rw, err.Error(), http.StatusRequestEntityTooLarge)
	case errors.Is(err, errBadSignature):
		http.Error(rw, err.Error(), http.StatusUnauthorized)
	default:
		http.Error(rw, err.Error(), http.StatusBadRequest)
	}
}

// HandleSlash serves POST ManagedSlashPath: verify the signature, parse the
// form-encoded command, ACK, and dispatch detached. There is no Socket Mode
// envelope id on this transport, so replay protection is the trigger_id dedup
// claim inside the processor. Unknown commands ACK-and-drop in the processor;
// a nil processor (slash not wired) ACKs without dispatch.
func (w *ManagedWebhook) HandleSlash(rw http.ResponseWriter, r *http.Request) {
	body, err := w.verifiedBody(r)
	if err != nil {
		writeWebhookError(rw, err)
		return
	}
	r.Body = io.NopCloser(strings.NewReader(string(body)))
	cmd, err := slack.SlashCommandParse(r)
	if err != nil {
		http.Error(rw, "cannot parse command", http.StatusBadRequest)
		return
	}
	rw.WriteHeader(http.StatusOK)
	if w.slash == nil {
		return
	}
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), managedWebhookTimeout)
		defer cancel()
		w.slash.HandleEnvelope(ctx, cmd, "")
	}()
}
// dispatchDetached normalizes one event_callback off the ACK path, mirroring
// slackChannel.dispatchEventsAPI. The bot identity comes from the resolved
// installation's stored config (not from a per-connection fixed id, since one
// webhook serves every managed team). A nil engine handle (tests) ACKs only.
func (w *ManagedWebhook) dispatchDetached(event slackevents.EventsAPIEvent) {
	if w.handle == nil {
		return
	}
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), managedWebhookTimeout)
		defer cancel()
		msg, ok := w.translate(ctx, event)
		if !ok {
			return
		}
		if err := w.handle(ctx, msg); err != nil {
			w.logger.WarnContext(ctx, "slack managed webhook: engine dispatch failed",
				"app_id", event.APIAppID, "error", err)
		}
	}()
}

// translate resolves the tenant and normalizes the inner event. ok=false means
// "nothing to ingest" (unknown team, revoked mapping, bot echo, subtype) —
// all ACK-and-drop, never an error Slack should retry.
func (w *ManagedWebhook) translate(ctx context.Context, event slackevents.EventsAPIEvent) (channel.InboundMessage, bool) {
	inst, err := lookupInstallation(ctx, w.q, event.APIAppID, event.TeamID)
	if err != nil {
		return channel.InboundMessage{}, false
	}
	if inst.Status != "active" {
		return channel.InboundMessage{}, false
	}
	botUserID := installBotUserID(inst.Config)
	mentionRe := compileMentionRe(botUserID)
	switch inner := event.InnerEvent.Data.(type) {
	case *slackevents.AppMentionEvent:
		return inboundFromAppMention(event, inner, botUserID, mentionRe)
	case *slackevents.MessageEvent:
		return inboundFromMessage(event, inner, botUserID, mentionRe)
	default:
		return channel.InboundMessage{}, false
	}
}

// installBotUserID reads the bot's own user id from a stored installation
// config, or "" when absent — mention detection then degrades to DMs and
// app_mention events only, same as an un-routable team.
func installBotUserID(configJSON json.RawMessage) string {
	var cfg installConfig
	if err := json.Unmarshal(configJSON, &cfg); err != nil {
		return ""
	}
	return cfg.BotUserID
}

// webhookChannel is the Supervisor-side half of a managed installation: it
// holds no connection (inbound arrives on the deployment webhook), so Connect
// parks until ctx ends and Send delivers over the Web API on the bot token.
// Outbound delivery, binding prompts, and history therefore work for managed
// installs exactly as they do for BYO ones; only the inbound link differs.
type webhookChannel struct {
	botUserID string
	botAPI    *slack.Client
	logger    *slog.Logger
}

func (c *webhookChannel) Type() channel.Type { return TypeSlack }

func (c *webhookChannel) Capabilities() channel.Capability {
	return channel.CapText | channel.CapThreadReply
}

// Connect parks: there is no per-installation link to run. It returns nil on
// ctx cancellation (graceful shutdown / lease loss) like any clean Connect
// exit, so the Supervisor neither reconnects nor backs off around it.
func (c *webhookChannel) Connect(ctx context.Context) error {
	<-ctx.Done()
	return nil
}

// Disconnect is a no-op: a parked channel owns no link to release. Mirrors
// slackChannel.Disconnect.
func (c *webhookChannel) Disconnect(_ context.Context) error { return nil }

// Send posts an outbound reply with this installation's bot token, reusing the
// shared slackSender — the same path slackChannel.Send takes.
func (c *webhookChannel) Send(ctx context.Context, out channel.OutboundMessage) (channel.SendResult, error) {
	return newSlackSender(credentials{BotUserID: c.botUserID}, c.botAPI, c.logger).Send(ctx, out)
}
