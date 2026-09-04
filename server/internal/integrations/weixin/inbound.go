package weixin

import (
	"encoding/json"
	"strconv"
	"strings"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel/engine"
)

// RawEvent is the platform-specific portion retained for resolver/replier
// use. The context token is never copied into the normalized Source or event
// envelope; it remains sealed in the session binding after acceptance.
type RawEvent struct {
	BotID        string `json:"bot_id"`
	EventType    string `json:"event_type"`
	ContextToken string `json:"context_token"`
}

// NormalizeInbound applies the verified iLink boundaries:
// direct text only, non-empty sender/context, and self-message rejection.
// A false result is a product drop and must not enter the shared router.
func NormalizeInbound(message WeixinMessage, botID string) (channel.InboundMessage, bool) {
	botID = strings.TrimSpace(botID)
	if message.MessageType != 1 || botID == "" || strings.TrimSpace(message.FromUserID) == "" ||
		message.FromUserID == botID || strings.TrimSpace(message.ContextToken) == "" ||
		strings.TrimSpace(message.GroupID) != "" {
		return channel.InboundMessage{}, false
	}
	parts := make([]string, 0, len(message.ItemList))
	for _, item := range message.ItemList {
		if item.Type == 1 && item.TextItem != nil {
			parts = append(parts, item.TextItem.Text)
		}
	}
	text := strings.TrimSpace(strings.Join(parts, "\n"))
	if text == "" {
		return channel.InboundMessage{}, false
	}
	commandText := text
	forceFresh := false
	if control, ok := engine.ParseControlCommand(text); ok && control.Kind == engine.ControlCommandFreshSession {
		text = control.Body
		forceFresh = true
	}
	messageID := ""
	switch {
	case message.MessageID != 0:
		messageID = strconv.FormatInt(message.MessageID, 10)
	case strings.TrimSpace(message.ClientID) != "":
		messageID = strings.TrimSpace(message.ClientID)
	default:
		messageID = strconv.FormatInt(message.Seq, 10) + ":" + message.FromUserID
	}
	raw, err := json.Marshal(RawEvent{BotID: botID, EventType: "message", ContextToken: message.ContextToken})
	if err != nil {
		return channel.InboundMessage{}, false
	}
	return channel.InboundMessage{
		EventID:   strconv.FormatInt(message.Seq, 10),
		MessageID: messageID,
		Source: channel.Source{
			ChannelType:    TypeWeixin,
			ChatID:         message.FromUserID,
			ChatType:       channel.ChatTypeP2P,
			SenderID:       message.FromUserID,
			SenderStableID: message.FromUserID,
		},
		Type:           channel.MsgTypeText,
		Text:           text,
		CommandText:    commandText,
		ForceFresh:     forceFresh,
		AddressedToBot: true,
		Raw:            raw,
	}, true
}
