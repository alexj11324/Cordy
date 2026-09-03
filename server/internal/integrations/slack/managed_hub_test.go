package slack

import (
	"context"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
	"time"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
)

func TestManagedAgentsCommandReentersTheSharedRouter(t *testing.T) {
	const secret = "hub-signing-fixture"
	received := make(chan channel.InboundMessage, 1)
	webhook, err := NewManagedWebhook(ManagedWebhookConfig{
		Queries: &fakeAppLookup{}, SigningSecret: secret,
		Handle: func(_ context.Context, msg channel.InboundMessage) error {
			received <- msg
			return nil
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	body := url.Values{
		"command": {"/agents"}, "text": {"2"}, "trigger_id": {"trigger-fixture"},
		"api_app_id": {"A1"}, "team_id": {"T1"}, "channel_id": {"D1"}, "user_id": {"U1"},
	}.Encode()
	header, _ := signSlackRequest(secret, body, time.Now())
	header.Set("Content-Type", "application/x-www-form-urlencoded")
	req := httptest.NewRequest(http.MethodPost, ManagedSlashPath, strings.NewReader(body))
	req.Header = header
	rec := httptest.NewRecorder()
	webhook.HandleSlash(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("signed command was not acknowledged: %d", rec.Code)
	}
	select {
	case msg := <-received:
		if msg.CommandText != "/agents 2" || msg.MessageID != "trigger-fixture" || msg.Source.ChatID != "D1" || msg.Source.SenderID != "U1" || !msg.AddressedToBot {
			t.Fatal("Hub command lost its selector, replay identity, sender or conversation")
		}
	case <-time.After(time.Second):
		t.Fatal("managed /agents was acknowledged but never entered workspace routing")
	}
}
