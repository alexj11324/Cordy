package weixin

import (
	"encoding/json"
	"testing"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
)

func TestNormalizeInboundPreservesProvenanceAndStripsOnlyClear(t *testing.T) {
	message := WeixinMessage{
		Seq:          42,
		MessageID:    99,
		FromUserID:   "wx-user",
		ToUserID:     "bot-id",
		MessageType:  1,
		ContextToken: "opaque-context",
		ItemList: []MessageItem{
			{Type: 1, TextItem: &TextItem{Text: "  /clear first line  "}},
			{Type: 1, TextItem: &TextItem{Text: "second line"}},
			{Type: 99, TextItem: &TextItem{Text: "ignored provider item"}},
		},
	}

	got, ok := NormalizeInbound(message, "bot-id")
	if !ok {
		t.Fatal("NormalizeInbound rejected a valid direct text message")
	}
	if got.EventID != "42" || got.MessageID != "99" {
		t.Fatalf("ids = %q/%q", got.EventID, got.MessageID)
	}
	if got.Source != (channel.Source{
		ChannelType:    TypeWeixin,
		ChatID:         "wx-user",
		ChatType:       channel.ChatTypeP2P,
		SenderID:       "wx-user",
		SenderStableID: "wx-user",
	}) {
		t.Fatalf("source = %#v", got.Source)
	}
	if got.Text != "first line\nsecond line" || got.CommandText != "/clear first line  \nsecond line" || !got.ForceFresh || !got.AddressedToBot {
		t.Fatalf("normalized text/command/fresh/addressed = %q/%q/%t/%t", got.Text, got.CommandText, got.ForceFresh, got.AddressedToBot)
	}
	if len(got.MediaRefs) != 0 || got.ReplyTo != nil || got.Type != channel.MsgTypeText {
		t.Fatalf("unexpected cross-channel fields: %#v", got)
	}
	var raw RawEvent
	if err := json.Unmarshal(got.Raw, &raw); err != nil {
		t.Fatal(err)
	}
	if raw.BotID != "bot-id" || raw.EventType != "message" || raw.ContextToken != "opaque-context" {
		t.Fatalf("raw provenance = %#v", raw)
	}
}

func TestNormalizeInboundUsesStableFallbackIDsAndRejectsUnsupportedMessages(t *testing.T) {
	base := WeixinMessage{
		Seq:          7,
		FromUserID:   "wx-user",
		MessageType:  1,
		ContextToken: "ctx",
		ItemList:     []MessageItem{{Type: 1, TextItem: &TextItem{Text: "hello"}}},
	}
	for _, test := range []struct {
		name string
		edit func(*WeixinMessage)
	}{
		{name: "self", edit: func(m *WeixinMessage) { m.FromUserID = "bot-id" }},
		{name: "group", edit: func(m *WeixinMessage) { m.GroupID = "group-id" }},
		{name: "wrong message type", edit: func(m *WeixinMessage) { m.MessageType = 2 }},
		{name: "missing context", edit: func(m *WeixinMessage) { m.ContextToken = "" }},
		{name: "missing sender", edit: func(m *WeixinMessage) { m.FromUserID = "" }},
		{name: "only unsupported item", edit: func(m *WeixinMessage) { m.ItemList = []MessageItem{{Type: 2, TextItem: &TextItem{Text: "image"}}} }},
	} {
		t.Run(test.name, func(t *testing.T) {
			message := base
			test.edit(&message)
			if _, ok := NormalizeInbound(message, "bot-id"); ok {
				t.Fatal("unsupported or unsafe message was accepted")
			}
		})
	}

	for _, test := range []struct {
		name      string
		clientID  string
		wantID    string
		wantFresh bool
		wantText  string
	}{
		{name: "client fallback", clientID: "client-1", wantID: "client-1", wantText: "hello"},
		{name: "sequence fallback", wantID: "7:wx-user", wantText: "hello"},
		{name: "new remains shared command", clientID: "client-2", wantID: "client-2", wantText: "/new next"},
	} {
		t.Run(test.name, func(t *testing.T) {
			message := base
			message.ClientID = test.clientID
			if test.name == "new remains shared command" {
				message.ItemList = []MessageItem{{Type: 1, TextItem: &TextItem{Text: "/new next"}}}
			}
			got, ok := NormalizeInbound(message, "bot-id")
			if !ok || got.MessageID != test.wantID || got.Text != test.wantText || got.ForceFresh != test.wantFresh {
				t.Fatalf("result = %#v, ok=%t", got, ok)
			}
		})
	}
}
