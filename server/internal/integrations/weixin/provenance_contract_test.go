package weixin

import (
	"encoding/base64"
	"encoding/json"
	"strings"
	"testing"

	"github.com/patchbay-ai/patchbay/server/internal/events"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

func TestSessionBindingSealsContextAndBindsSenderToDirectChat(t *testing.T) {
	sealedInput := ""
	binder := &sessionBinder{seal: func(value []byte) ([]byte, error) {
		sealedInput = string(value)
		return append([]byte("sealed:"), value...), nil
	}}
	message := channel.InboundMessage{
		Source: channel.Source{ChannelType: TypeWeixin, ChatID: "wx-user", ChatType: channel.ChatTypeP2P, SenderID: "wx-user"},
		Raw:    mustJSON(t, RawEvent{BotID: "bot-id", EventType: "message", ContextToken: "opaque-context"}),
	}
	config, err := binder.bindingFor(message)
	if err != nil {
		t.Fatal(err)
	}
	if sealedInput != "opaque-context" || strings.Contains(string(config), "opaque-context") {
		t.Fatalf("binding config leaked plaintext context: %s", config)
	}
	var got weixinBindingConfig
	if err := json.Unmarshal(config, &got); err != nil {
		t.Fatal(err)
	}
	decoded, err := base64.StdEncoding.DecodeString(got.ContextTokenEncrypted)
	if err != nil {
		t.Fatal(err)
	}
	if string(decoded) != "sealed:opaque-context" || got.UserID != "wx-user" {
		t.Fatalf("binding config = %#v, decoded context = %q", got, decoded)
	}
}

func TestSessionBindingFailsClosedForMissingRawOrSealer(t *testing.T) {
	message := channel.InboundMessage{Source: channel.Source{SenderID: "wx-user"}}
	if _, err := (&sessionBinder{seal: func([]byte) ([]byte, error) { return nil, nil }}).bindingFor(message); err == nil {
		t.Fatal("missing Raw event was accepted")
	}
	message.Raw = mustJSON(t, RawEvent{BotID: "bot-id", EventType: "message", ContextToken: "ctx"})
	if _, err := (&sessionBinder{}).bindingFor(message); err == nil {
		t.Fatal("missing context sealer was accepted")
	}
}

func TestWeixinDirectSourceFenceRejectsCrossChannelAndCrossChatEnvelopes(t *testing.T) {
	base := channel.InboundMessage{
		Source: channel.Source{ChannelType: TypeWeixin, ChatID: "wx-user", ChatType: channel.ChatTypeP2P, SenderID: "wx-user"},
		Raw:    mustJSON(t, RawEvent{BotID: "bot-id", EventType: "message", ContextToken: "ctx"}),
	}
	if !validDirectSource(base) {
		t.Fatal("valid direct source was rejected")
	}
	for _, edit := range []func(*channel.InboundMessage){
		func(m *channel.InboundMessage) { m.Source.ChannelType = channel.Type("wecom") },
		func(m *channel.InboundMessage) { m.Source.ChatType = channel.ChatTypeGroup },
		func(m *channel.InboundMessage) { m.Source.ChatID = "other-chat" },
		func(m *channel.InboundMessage) { m.Source.SenderID = "other-user" },
	} {
		message := base
		edit(&message)
		if validDirectSource(message) {
			t.Fatalf("unsafe direct source accepted: %#v", message.Source)
		}
		if _, err := (&sessionBinder{seal: func([]byte) ([]byte, error) { return []byte("sealed"), nil }}).bindingFor(message); err == nil {
			t.Fatal("unsafe source reached binding config")
		}
	}
}

func TestOutboundEventHelpersHonorTaskFailureRetryAndEnvelopePrecedence(t *testing.T) {
	taskID := "123e4567-e89b-12d3-a456-426614174000"
	chatID := "123e4567-e89b-12d3-a456-426614174001"
	if got, ok := eventUUID(taskID, map[string]any{"task_id": "bad"}, "task_id"); !ok || got.String() != taskID {
		t.Fatalf("envelope task id = %v/%t", got, ok)
	}
	if got, ok := eventUUID("", map[string]any{"chat_session_id": chatID}, "chat_session_id"); !ok || got.String() != chatID {
		t.Fatalf("payload chat id = %v/%t", got, ok)
	}
	if got, ok := eventUUID("", protocol.ChatDonePayload{TaskID: taskID}, "task_id"); !ok || got.String() != taskID {
		t.Fatalf("typed payload task id = %v/%t", got, ok)
	}
	if _, ok := eventUUID("not-a-uuid", nil, "task_id"); ok {
		t.Fatal("invalid UUID was accepted")
	}

	if got := eventContent(events.Event{Type: protocol.EventChatDone, Payload: protocol.ChatDonePayload{Content: "answer"}}); got != "answer" {
		t.Fatalf("chat-done content = %q", got)
	}
	if got := eventContent(events.Event{Type: protocol.EventTaskFailed, Payload: map[string]any{"retry_pending": true}}); got != "" {
		t.Fatalf("retry-pending failure = %q", got)
	}
	if got := eventContent(events.Event{Type: protocol.EventTaskFailed, Payload: map[string]any{"retry_pending": false}}); got != weixinTaskFailureText {
		t.Fatalf("terminal failure = %q", got)
	}
}

func TestResolverSetUsesWeixinOriginAndTextOnlyCapability(t *testing.T) {
	if OriginWeixinChat == "" || TypeWeixin != channel.Type("weixin") {
		t.Fatalf("unexpected Weixin constants: %q/%q", OriginWeixinChat, TypeWeixin)
	}
	set := NewResolverSet(nil, nil, nil, nil, nil)
	if set.OriginType != OriginWeixinChat || set.Installation == nil || set.Identity == nil || set.Dedup == nil || set.Session == nil {
		t.Fatalf("resolver set is incomplete: %#v", set)
	}
}

func mustJSON(t *testing.T, value any) []byte {
	t.Helper()
	encoded, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	return encoded
}
