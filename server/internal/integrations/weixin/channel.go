package weixin

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"sync"
	"time"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
	"github.com/patchbay-ai/patchbay/server/internal/util"
)

const receiveRetryDelay = 2 * time.Second

const unsupportedMessageText = "This WeChat connection currently supports text messages only."

// weixinChannel owns one QR-installed iLink bot. The provider is HTTP long
// poll rather than WeCom's corporate WebSocket; the shared Supervisor still
// supplies the installation lease and reconnect lifecycle.
type weixinChannel struct {
	installationID pgtype.UUID
	botID          string
	client         *Client
	pool           *pgxpool.Pool
	handler        channel.InboundHandler
	logger         *slog.Logger

	mu            sync.RWMutex
	contextByChat map[string]string
}

var _ channel.Channel = (*weixinChannel)(nil)

func (c *weixinChannel) Type() channel.Type { return TypeWeixin }

func (c *weixinChannel) Capabilities() channel.Capability {
	return channel.CapText
}

func (c *weixinChannel) Disconnect(context.Context) error {
	c.mu.Lock()
	clear(c.contextByChat)
	c.mu.Unlock()
	return nil
}

// Send is retained for the shared Channel contract. iLink needs the opaque
// context_token issued on an inbound turn, so the channel caches the latest
// token for each direct chat during its receive loop. The durable outbound
// subscriber uses the sealed binding config and does not depend on this
// process-local cache after a restart.
func (c *weixinChannel) Send(ctx context.Context, out channel.OutboundMessage) (channel.SendResult, error) {
	if out.ChatID == "" {
		return channel.SendResult{}, errors.New("weixin: outbound chat id is empty")
	}
	c.mu.RLock()
	contextToken := c.contextByChat[out.ChatID]
	c.mu.RUnlock()
	if contextToken == "" {
		return channel.SendResult{}, errors.New("weixin: no context token for outbound chat")
	}
	ids, err := c.client.SendText(ctx, out.ChatID, contextToken, out.Text)
	if err != nil {
		return channel.SendResult{}, err
	}
	return channel.SendResult{MessageID: firstString(ids), MessageIDs: ids}, nil
}

func (c *weixinChannel) Connect(ctx context.Context) error {
	if c.handler == nil {
		return errors.New("weixin: inbound handler not configured")
	}
	if c.pool == nil {
		return errors.New("weixin: receive cursor store not configured")
	}
	cursor, err := loadReceiveCursor(ctx, c.pool, c.installationID)
	if err != nil {
		return err
	}
	for {
		if ctx.Err() != nil {
			return nil
		}
		updates, err := c.client.GetUpdates(ctx, cursor)
		if err != nil {
			if ctx.Err() != nil {
				return nil
			}
			c.logger.WarnContext(ctx, "weixin: getupdates failed", "bot_id", c.botID, "error", err)
			channel.ReportRuntime(ctx, channel.RuntimeObservation{State: "degraded", ErrorCode: "poll_failed"})
			if !sleepContext(ctx, receiveRetryDelay) {
				return nil
			}
			return fmt.Errorf("weixin: getupdates: %w", err)
		}
		channel.ReportConnected(ctx)
		for _, message := range updates.Messages {
			if err := c.dispatch(ctx, message); err != nil {
				if ctx.Err() != nil {
					return nil
				}
				return err
			}
		}
		if updates.NextCursor != "" {
			cursor = updates.NextCursor
			if err := saveReceiveCursor(ctx, c.pool, c.installationID, cursor); err != nil {
				return err
			}
		}
	}
}

func (c *weixinChannel) dispatch(ctx context.Context, message WeixinMessage) error {
	normalized, ok := NormalizeInbound(message, c.botID)
	if !ok {
		if shouldNotifyUnsupported(message, c.botID) {
			if _, err := c.client.SendText(ctx, message.FromUserID, message.ContextToken, unsupportedMessageText); err != nil {
				return fmt.Errorf("weixin: send unsupported-message notice: %w", err)
			}
		}
		return nil
	}
	var raw RawEvent
	if err := json.Unmarshal(normalized.Raw, &raw); err != nil || raw.ContextToken == "" {
		return errors.New("weixin: normalized event has no context token")
	}
	c.mu.Lock()
	c.contextByChat[normalized.Source.ChatID] = raw.ContextToken
	c.mu.Unlock()
	if err := c.handler(ctx, normalized); err != nil {
		return fmt.Errorf("weixin: dispatch inbound message: %w", err)
	}
	return nil
}

func shouldNotifyUnsupported(message WeixinMessage, botID string) bool {
	return message.MessageType == 1 && message.FromUserID != "" && message.FromUserID != botID &&
		message.GroupID == "" && message.ContextToken != "" && len(message.ItemList) > 0
}

func firstString(values []string) string {
	if len(values) == 0 {
		return ""
	}
	return values[0]
}

func sleepContext(ctx context.Context, duration time.Duration) bool {
	timer := time.NewTimer(duration)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return false
	case <-timer.C:
		return true
	}
}

// ChannelDeps are the dependencies captured by the per-installation factory.
// The handler itself is injected by engine.Supervisor through channel.Config.
type ChannelDeps struct {
	Decrypt    Decrypter
	Pool       *pgxpool.Pool
	HTTPClient *http.Client
	Logger     *slog.Logger
}

// RegisterWeixin adds the native iLink factory to the shared channel registry.
func RegisterWeixin(reg *channel.Registry, deps ChannelDeps) {
	if reg == nil {
		return
	}
	logger := deps.Logger
	if logger == nil {
		logger = slog.Default()
	}
	reg.Register(TypeWeixin, func(cfg channel.Config) (channel.Channel, error) {
		if cfg.ID.Valid == false {
			return nil, errors.New("weixin: installation id is required")
		}
		if deps.Pool == nil {
			return nil, errors.New("weixin: database pool not configured")
		}
		if cfg.Handler == nil {
			return nil, errors.New("weixin: inbound handler not configured")
		}
		credentials, err := DecodeCredentials(cfg.Raw, deps.Decrypt)
		if err != nil {
			return nil, err
		}
		baseURL, err := ValidateProviderBaseURL(credentials.BaseURL)
		if err != nil {
			return nil, err
		}
		return &weixinChannel{
			installationID: cfg.ID,
			botID:          credentials.BotID,
			client:         NewClient(baseURL, credentials.BotToken, deps.HTTPClient),
			pool:           deps.Pool,
			handler:        cfg.Handler,
			logger:         logger.With("installation_id", util.UUIDToString(cfg.ID)),
			contextByChat:  make(map[string]string),
		}, nil
	})
}
