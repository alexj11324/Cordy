package weixin

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"

	"github.com/patchbay-ai/patchbay/server/internal/events"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel/engine"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

const weixinTaskFailureText = "❌ The agent run failed. Please try again."

type outboundQueries interface {
	GetChannelTaskDelivery(context.Context, pgtype.UUID) (db.ChannelTaskDelivery, error)
	GetAgentTask(context.Context, pgtype.UUID) (db.AgentTaskQueue, error)
	TaskHasChannelIngestedMessages(context.Context, pgtype.UUID) (bool, error)
	GetChannelInstallation(context.Context, db.GetChannelInstallationParams) (db.ChannelInstallation, error)
	SetChatMessageChannelOutboundProvenanceByTask(context.Context, db.SetChatMessageChannelOutboundProvenanceByTaskParams) (int64, error)
	RecordChannelOutboundMessage(context.Context, db.RecordChannelOutboundMessageParams) error
}

type Outbound struct {
	q       outboundQueries
	decrypt Decrypter
	logger  *slog.Logger
}

func NewOutbound(q outboundQueries, decrypt Decrypter, logger *slog.Logger) *Outbound {
	if logger == nil {
		logger = slog.Default()
	}
	return &Outbound{q: q, decrypt: decrypt, logger: logger}
}

func (o *Outbound) Register(bus *events.Bus) {
	if bus == nil {
		return
	}
	bus.Subscribe(protocol.EventChatDone, o.handleEvent)
	bus.Subscribe(protocol.EventTaskFailed, o.handleEvent)
}

func (o *Outbound) handleEvent(event events.Event) {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	if err := o.process(ctx, event); err != nil {
		o.logger.WarnContext(ctx, "weixin outbound delivery failed", "error", err)
	}
}

func (o *Outbound) process(ctx context.Context, event events.Event) error {
	if o == nil || o.q == nil {
		return errors.New("weixin: outbound query is not configured")
	}
	taskID, ok := eventUUID(event.TaskID, event.Payload, "task_id")
	if !ok {
		return nil
	}
	delivery, err := o.q.GetChannelTaskDelivery(ctx, taskID)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil
	}
	if err != nil {
		return fmt.Errorf("weixin: load task delivery: %w", err)
	}
	if delivery.ChannelType != string(TypeWeixin) || delivery.ChannelChatID == "" || !delivery.InstallationID.Valid || !delivery.BindingID.Valid {
		return nil
	}
	task, err := o.q.GetAgentTask(ctx, taskID)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil
	}
	if err != nil {
		return fmt.Errorf("weixin: load agent task: %w", err)
	}
	deliver, err := engine.TaskInputIsChannelIngested(ctx, o.q, task)
	if err != nil {
		return fmt.Errorf("weixin: classify task origin: %w", err)
	}
	if !deliver {
		return nil
	}
	text := eventContent(event)
	if text == "" {
		return nil
	}
	installation, err := o.q.GetChannelInstallation(ctx, db.GetChannelInstallationParams{
		ID: delivery.InstallationID, ChannelType: string(TypeWeixin),
	})
	if errors.Is(err, pgx.ErrNoRows) {
		return nil
	}
	if err != nil {
		return fmt.Errorf("weixin: load installation: %w", err)
	}
	if installation.Status != "installed" || installation.WorkspaceID.Valid == false {
		return nil
	}
	if event.WorkspaceID != "" && event.WorkspaceID != util.UUIDToString(installation.WorkspaceID) {
		return nil
	}
	credentials, err := DecodeCredentials(installation.Config, o.decrypt)
	if err != nil {
		return fmt.Errorf("weixin: decode installation credentials: %w", err)
	}
	baseURL, err := ValidateProviderBaseURL(credentials.BaseURL)
	if err != nil {
		return err
	}
	var route weixinBindingConfig
	if err := json.Unmarshal(delivery.Config, &route); err != nil {
		return fmt.Errorf("weixin: decode chat binding config: %w", err)
	}
	if strings.TrimSpace(route.UserID) == "" || route.UserID != delivery.ChannelChatID || strings.TrimSpace(route.ContextTokenEncrypted) == "" {
		return errors.New("weixin: chat binding has no fenced outbound context")
	}
	ciphertext, err := base64.StdEncoding.DecodeString(stripWhitespace(route.ContextTokenEncrypted))
	if err != nil {
		return fmt.Errorf("weixin: decode context token: %w", err)
	}
	plaintext := ciphertext
	if o.decrypt != nil {
		plaintext, err = o.decrypt(ciphertext)
		if err != nil {
			return fmt.Errorf("weixin: decrypt context token: %w", err)
		}
	}
	contextToken := strings.TrimSpace(string(plaintext))
	if contextToken == "" {
		return errors.New("weixin: context token is empty")
	}
	ids, err := NewClient(baseURL, credentials.BotToken, nil).SendText(ctx, route.UserID, contextToken, text)
	if err != nil {
		return fmt.Errorf("weixin: send agent reply: %w", err)
	}
	if len(ids) == 0 {
		return errors.New("weixin: provider returned no outbound correlation id")
	}
	rows, err := o.q.SetChatMessageChannelOutboundProvenanceByTask(ctx, db.SetChatMessageChannelOutboundProvenanceByTaskParams{
		ChannelType: pgtype.Text{String: string(TypeWeixin), Valid: true}, InstallationID: delivery.InstallationID,
		ChannelChatID: pgtype.Text{String: delivery.ChannelChatID, Valid: true}, MessageIds: ids, TaskID: taskID,
	})
	if err != nil {
		return fmt.Errorf("weixin: record reply provenance: %w", err)
	}
	if rows != 1 {
		return fmt.Errorf("weixin: record reply provenance updated %d assistant rows, want 1", rows)
	}
	for _, id := range ids {
		if err := o.q.RecordChannelOutboundMessage(ctx, db.RecordChannelOutboundMessageParams{
			OutboundInstallationID: installation.ID, OutboundChannelType: string(TypeWeixin), OutboundMessageID: id,
			OutboundBindingID: delivery.BindingID, OutboundRouteRevision: delivery.RouteRevision, OutboundTaskID: taskID,
			OutboundKind: "task_reply",
		}); err != nil {
			return fmt.Errorf("weixin: record outbound message: %w", err)
		}
	}
	return nil
}

func weixinBindingFromTaskDelivery(delivery db.ChannelTaskDelivery) db.ChannelChatSessionBinding {
	return db.ChannelChatSessionBinding{
		ID: delivery.BindingID, InstallationID: delivery.InstallationID,
		ChannelType: delivery.ChannelType, ChannelChatID: delivery.ChannelChatID,
		ChatType: delivery.ChatType, RouteRevision: delivery.RouteRevision,
		Config: delivery.Config,
	}
}

func eventUUID(envelope string, payload any, key string) (pgtype.UUID, bool) {
	raw := strings.TrimSpace(envelope)
	if raw == "" {
		if values, ok := payload.(map[string]any); ok {
			raw, _ = values[key].(string)
		} else if done, ok := payload.(protocol.ChatDonePayload); ok {
			switch key {
			case "task_id":
				raw = done.TaskID
			case "chat_session_id":
				raw = done.ChatSessionID
			}
		}
	}
	id, err := util.ParseUUID(raw)
	return id, err == nil && id.Valid
}

func eventContent(event events.Event) string {
	if event.Type == protocol.EventTaskFailed {
		if values, ok := event.Payload.(map[string]any); ok {
			if retryPending, _ := values["retry_pending"].(bool); retryPending {
				return ""
			}
		}
		return weixinTaskFailureText
	}
	switch payload := event.Payload.(type) {
	case protocol.ChatDonePayload:
		return payload.Content
	case map[string]any:
		value, _ := payload["content"].(string)
		return value
	default:
		return ""
	}
}
