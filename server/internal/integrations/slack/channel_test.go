package slack

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"testing"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/slack-go/slack"

	"github.com/orvilo-ai/orvilo/server/internal/integrations/channel"
)

func TestSlackFactoryCarriesNativeAutomationHookAndInstallation(t *testing.T) {
	installationID := pgtype.UUID{Bytes: [16]byte{1, 2, 3}, Valid: true}
	hookCalled := false
	factory := newSlackFactory(ChannelDeps{
		OnNativeEvent: func(ctx context.Context, id pgtype.UUID, body []byte) {
			hookCalled = true
			_ = ctx
			_ = id
			_ = body
		},
	})
	ch, err := factory(channel.Config{
		ID:      installationID,
		Raw:     []byte(`{"app_id":"A1","bot_user_id":"U1","app_token_encrypted":"eGFwcC0x","bot_token_encrypted":"eG94Yi0x"}`),
		Handler: func(context.Context, channel.InboundMessage) error { return nil },
	})
	if err != nil {
		t.Fatalf("factory: %v", err)
	}
	got, ok := ch.(*slackChannel)
	if !ok {
		t.Fatalf("channel type = %T, want *slackChannel", ch)
	}
	if got.installationID != installationID {
		t.Fatalf("installation id = %v, want %v", got.installationID, installationID)
	}
	if got.onNativeEvent == nil {
		t.Fatal("native automation hook was not carried into Socket Mode channel")
	}
	got.onNativeEvent(context.Background(), installationID, []byte(`{}`))
	if !hookCalled {
		t.Fatal("native automation hook did not execute")
	}
}

func TestChunkMessage(t *testing.T) {
	if got := chunkMessage("short", 100); len(got) != 1 || got[0] != "short" {
		t.Errorf("short message should be one chunk: %v", got)
	}
	long := make([]rune, 250)
	for i := range long {
		long[i] = 'a'
	}
	chunks := chunkMessage(string(long), 100)
	if len(chunks) != 3 {
		t.Fatalf("250 runes / 100 = 3 chunks, got %d", len(chunks))
	}
	if len([]rune(chunks[0])) != 100 || len([]rune(chunks[2])) != 50 {
		t.Errorf("chunk sizes wrong: %d / %d", len([]rune(chunks[0])), len([]rune(chunks[2])))
	}
}

func TestSendWithMetadataTagsEverySlackMessage(t *testing.T) {
	var gotForm url.Values
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_ = r.ParseForm()
		gotForm = r.PostForm
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"ok":true,"channel":"C123","ts":"1.1"}`))
	}))
	defer srv.Close()
	c := newSlackSender(credentials{TeamID: "T1"}, slack.New("xoxb-test", slack.OptionAPIURL(srv.URL+"/")), nil)
	metadata := outboundMetadata(uid(3), 2, "control_ack")
	if _, err := c.SendWithMetadata(context.Background(), channel.OutboundMessage{ChatID: "C123", Text: "started"}, metadata); err != nil {
		t.Fatalf("SendWithMetadata: %v", err)
	}
	var got slack.SlackMetadata
	if err := json.Unmarshal([]byte(gotForm.Get("metadata")), &got); err != nil {
		t.Fatalf("decode metadata %q: %v", gotForm.Get("metadata"), err)
	}
	if got.EventType != slackOutboundMetadataEvent || got.EventPayload["kind"] != "control_ack" {
		t.Fatalf("metadata=%+v", got)
	}
}

func TestSend(t *testing.T) {
	var gotForm url.Values
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_ = r.ParseForm()
		gotForm = r.PostForm
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"ok":true,"channel":"C123","ts":"1700000000.111111"}`))
	}))
	defer srv.Close()

	api := slack.New("xoxb-test", slack.OptionAPIURL(srv.URL+"/"))
	c := newSlackSender(credentials{TeamID: "T1"}, api, nil)

	res, err := c.Send(context.Background(), channel.OutboundMessage{
		ChatID:   "C123",
		Text:     "reply body",
		ThreadID: "1700000000.000400",
	})
	if err != nil {
		t.Fatalf("Send: %v", err)
	}
	if res.MessageID != "1700000000.111111" {
		t.Errorf("MessageID = %q", res.MessageID)
	}
	if len(res.MessageIDs) != 1 || res.MessageIDs[0] != "1700000000.111111" {
		t.Errorf("MessageIDs = %v", res.MessageIDs)
	}
	if gotForm.Get("channel") != "C123" || gotForm.Get("text") != "reply body" {
		t.Errorf("posted channel/text = %q / %q", gotForm.Get("channel"), gotForm.Get("text"))
	}
	if gotForm.Get("thread_ts") != "1700000000.000400" {
		t.Errorf("thread_ts = %q, want the inbound thread", gotForm.Get("thread_ts"))
	}
}

// TestSend_AppliesMrkdwn guards the wiring: Send must run the agent's Markdown
// through formatMrkdwn before posting, so Slack renders it instead of showing
// literal markup. (The converter itself is covered in mrkdwn_test.go.)
func TestSend_AppliesMrkdwn(t *testing.T) {
	var gotText string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_ = r.ParseForm()
		gotText = r.PostForm.Get("text")
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"ok":true,"channel":"C1","ts":"1.1"}`))
	}))
	defer srv.Close()

	api := slack.New("xoxb-test", slack.OptionAPIURL(srv.URL+"/"))
	c := newSlackSender(credentials{TeamID: "T1"}, api, nil)

	if _, err := c.Send(context.Background(), channel.OutboundMessage{
		ChatID: "C1",
		Text:   "**bold** see [docs](http://x.com)",
	}); err != nil {
		t.Fatalf("Send: %v", err)
	}
	if gotText != "*bold* see <http://x.com|docs>" {
		t.Errorf("Send must convert Markdown to mrkdwn before posting, got %q", gotText)
	}
}

func TestOutboundThreadTS(t *testing.T) {
	if got := outboundThreadTS(channel.OutboundMessage{ReplyTo: "111.1", ThreadID: "222.2"}); got != "111.1" {
		t.Errorf("explicit ReplyTo should win: %q", got)
	}
	if got := outboundThreadTS(channel.OutboundMessage{ThreadID: "222.2"}); got != "222.2" {
		t.Errorf("thread fallback: %q", got)
	}
	if got := outboundThreadTS(channel.OutboundMessage{}); got != "" {
		t.Errorf("top-level send has no thread: %q", got)
	}
}
