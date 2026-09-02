package slack

import (
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/slack-go/slack"
	"github.com/slack-go/slack/slackevents"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

type fakeAppLookup struct {
	rows map[string]db.ChannelInstallation
	err  error
}

func (f *fakeAppLookup) GetChannelInstallationByAppID(_ context.Context, arg db.GetChannelInstallationByAppIDParams) (db.ChannelInstallation, error) {
	if f.err != nil {
		return db.ChannelInstallation{}, f.err
	}
	row, ok := f.rows[arg.AppID]
	if !ok {
		return db.ChannelInstallation{}, pgx.ErrNoRows
	}
	return row, nil
}

func managedRow(appIDKey string) db.ChannelInstallation {
	return db.ChannelInstallation{
		Status: "active",
		Config: []byte(fmt.Sprintf(`{"app_id":%q,"api_app_id":"A1","team_id":"T1","bot_user_id":"UBOT"}`, appIDKey)),
	}
}

func signSlackRequest(secret, body string, ts time.Time) (http.Header, string) {
	stamp := strconv.FormatInt(ts.Unix(), 10)
	mac := hmac.New(sha256.New, []byte(secret))
	mac.Write([]byte("v0:" + stamp + ":" + body))
	header := http.Header{}
	header.Set("X-Slack-Request-Timestamp", stamp)
	header.Set("X-Slack-Signature", "v0="+hex.EncodeToString(mac.Sum(nil)))
	return header, body
}

func TestLookupInstallation_PrefersDirectAppID(t *testing.T) {
	q := &fakeAppLookup{rows: map[string]db.ChannelInstallation{"A1": managedRow("A1")}}
	got, err := lookupInstallation(context.Background(), q, "A1", "T1")
	if err != nil {
		t.Fatalf("lookup: %v", err)
	}
	var cfg installConfig
	if err := json.Unmarshal(got.Config, &cfg); err != nil {
		t.Fatalf("decode config: %v", err)
	}
	if cfg.AppID != "A1" {
		t.Fatalf("direct hit must win, routed to %q", cfg.AppID)
	}
}

func TestLookupInstallation_FallsBackToTenantComposite(t *testing.T) {
	q := &fakeAppLookup{rows: map[string]db.ChannelInstallation{"A1:T1": managedRow("A1:T1")}}
	got, err := lookupInstallation(context.Background(), q, "A1", "T1")
	if err != nil {
		t.Fatalf("lookup: %v", err)
	}
	var cfg installConfig
	if err := json.Unmarshal(got.Config, &cfg); err != nil {
		t.Fatalf("decode config: %v", err)
	}
	if cfg.AppID != "A1:T1" {
		t.Fatalf("managed install must resolve via composite, got %q", cfg.AppID)
	}
}

func TestLookupInstallation_ForeignTeamRefused(t *testing.T) {
	q := &fakeAppLookup{rows: map[string]db.ChannelInstallation{
		"A1":    managedRow("A1"),
		"A1:T1": managedRow("A1:T1"),
	}}
	if _, err := lookupInstallation(context.Background(), q, "A1", "T9"); err == nil {
		t.Fatal("event from a foreign team sharing the app must be refused")
	}
	if _, err := lookupInstallation(context.Background(), q, "A9", "T9"); err == nil {
		t.Fatal("unknown app must be refused")
	}
}

func TestManagedWebhook_ChallengeAndSignature(t *testing.T) {
	secret := "signing-secret"
	webhook, err := NewManagedWebhook(ManagedWebhookConfig{Queries: &fakeAppLookup{}, SigningSecret: secret})
	if err != nil {
		t.Fatalf("new webhook: %v", err)
	}
	// url_verification echoes the challenge after a valid signature.
	challengeBody := `{"token":"x","challenge":"abc123","type":"url_verification"}`
	header, _ := signSlackRequest(secret, challengeBody, time.Now())
	req := httptest.NewRequest(http.MethodPost, ManagedEventsPath, strings.NewReader(challengeBody))
	req.Header = header
	rec := httptest.NewRecorder()
	webhook.HandleEvents(rec, req)
	if rec.Code != http.StatusOK || rec.Body.String() != "abc123" {
		t.Fatalf("challenge: code=%d body=%q", rec.Code, rec.Body.String())
	}
	// Forged signature is refused before parsing.
	badReq := httptest.NewRequest(http.MethodPost, ManagedEventsPath, strings.NewReader(challengeBody))
	badReq.Header.Set("X-Slack-Request-Timestamp", strconv.FormatInt(time.Now().Unix(), 10))
	badReq.Header.Set("X-Slack-Signature", "v0=deadbeef")
	badRec := httptest.NewRecorder()
	webhook.HandleEvents(badRec, badReq)
	if badRec.Code != http.StatusUnauthorized {
		t.Fatalf("forged signature: code=%d, want 401", badRec.Code)
	}
	// Stale signature is refused even when the HMAC itself is well-formed.
	staleHeader, _ := signSlackRequest(secret, challengeBody, time.Now().Add(-10*time.Minute))
	staleReq := httptest.NewRequest(http.MethodPost, ManagedEventsPath, strings.NewReader(challengeBody))
	staleReq.Header = staleHeader
	staleRec := httptest.NewRecorder()
	webhook.HandleEvents(staleRec, staleReq)
	if staleRec.Code != http.StatusUnauthorized {
		t.Fatalf("stale signature: code=%d, want 401", staleRec.Code)
	}
}

func TestManagedWebhook_NoSecretFailsClosed(t *testing.T) {
	webhook, err := NewManagedWebhook(ManagedWebhookConfig{Queries: &fakeAppLookup{}})
	if err != nil {
		t.Fatalf("new webhook: %v", err)
	}
	req := httptest.NewRequest(http.MethodPost, ManagedEventsPath, strings.NewReader(`{}`))
	rec := httptest.NewRecorder()
	webhook.HandleEvents(rec, req)
	if rec.Code != http.StatusServiceUnavailable {
		t.Fatalf("unconfigured webhook: code=%d, want 503", rec.Code)
	}
}

func testDMEvent(teamID, appID string) slackevents.EventsAPIEvent {
	return slackevents.EventsAPIEvent{
		TeamID:   teamID,
		APIAppID: appID,
		Type:     slackevents.CallbackEvent,
		InnerEvent: slackevents.EventsAPIInnerEvent{
			Type: "message",
			Data: &slackevents.MessageEvent{
				Type: "message", User: "U1", Text: "hi",
				TimeStamp: "1700000000.000200", Channel: "D1",
				EventTimeStamp: "1700000000.000200",
			},
		},
	}
}

func TestManagedWebhook_TranslateDM(t *testing.T) {
	q := &fakeAppLookup{rows: map[string]db.ChannelInstallation{"A1:T1": managedRow("A1:T1")}}
	webhook, err := NewManagedWebhook(ManagedWebhookConfig{Queries: q, SigningSecret: "s"})
	if err != nil {
		t.Fatalf("new webhook: %v", err)
	}
	msg, ok := webhook.translate(context.Background(), testDMEvent("T1", "A1"))
	if !ok {
		t.Fatal("DM from a bound team must ingest")
	}
	if !msg.AddressedToBot || msg.Source.ChatType != channel.ChatTypeP2P {
		t.Fatalf("DM must be addressed p2p: %+v", msg.Source)
	}
}

func TestManagedWebhook_TranslateUnknownTeamDrops(t *testing.T) {
	q := &fakeAppLookup{rows: map[string]db.ChannelInstallation{"A1:T1": managedRow("A1:T1")}}
	webhook, err := NewManagedWebhook(ManagedWebhookConfig{Queries: q, SigningSecret: "s"})
	if err != nil {
		t.Fatalf("new webhook: %v", err)
	}
	if _, ok := webhook.translate(context.Background(), testDMEvent("T9", "A1")); ok {
		t.Fatal("foreign team must ACK-and-drop, never ingest")
	}
}

func TestManagedWebhook_CallbackAcksAndIgnoresUnknown(t *testing.T) {
	webhook, err := NewManagedWebhook(ManagedWebhookConfig{Queries: &fakeAppLookup{}, SigningSecret: "s3cr3t"})
	if err != nil {
		t.Fatalf("new webhook: %v", err)
	}
	// reaction_added is not ingested, but the delivery still ACKs so Slack
	// does not retry it.
	body := `{"token":"x","team_id":"T1","api_app_id":"A1","event":{"type":"reaction_added","user":"U1","item":{"type":"message","channel":"C1","ts":"1.2"},"event_ts":"1.3"},"type":"event_callback"}`
	header, _ := signSlackRequest("s3cr3t", body, time.Now())
	req := httptest.NewRequest(http.MethodPost, ManagedEventsPath, strings.NewReader(body))
	req.Header = header
	rec := httptest.NewRecorder()
	webhook.HandleEvents(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("unhandled inner event: code=%d, want ACK", rec.Code)
	}
}

func TestWebhookFactory_ParksManagedInstall(t *testing.T) {
	factory := newSlackFactory(ChannelDeps{})
	ch, err := factory(channel.Config{
		Type: TypeSlack,
		Raw:  json.RawMessage(`{"app_id":"A1:T1","api_app_id":"A1","team_id":"T1","bot_user_id":"UBOT","bot_token_encrypted":"","transport":"webhook"}`),
	})
	if err != nil {
		t.Fatalf("webhook transport must build, got: %v", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if err := ch.Connect(ctx); err != nil {
		t.Fatalf("parked connect must exit cleanly, got: %v", err)
	}
	if err := ch.Disconnect(context.Background()); err != nil {
		t.Fatalf("disconnect must be a no-op nil, got: %v", err)
	}
}

type stubSlashEnqueuer struct {
	cmds []slashCmd
}

type slashCmd struct {
	cmd        slack.SlashCommand
	envelopeID string
}

func (s *stubSlashEnqueuer) HandleEnvelope(_ context.Context, cmd slack.SlashCommand, envelopeID string) {
	s.cmds = append(s.cmds, slashCmd{cmd: cmd, envelopeID: envelopeID})
}

func TestManagedWebhook_SlashDispatchesDetached(t *testing.T) {
	stub := &stubSlashEnqueuer{}
	webhook, err := NewManagedWebhook(ManagedWebhookConfig{Queries: &fakeAppLookup{}, Slash: stub, SigningSecret: "s3cr3t"})
	if err != nil {
		t.Fatalf("new webhook: %v", err)
	}
	form := url.Values{}
	form.Set("command", "/issue")
	form.Set("text", "the login button does nothing")
	form.Set("user_id", "U1")
	form.Set("team_id", "T1")
	form.Set("api_app_id", "A1")
	form.Set("channel_id", "C1")
	form.Set("trigger_id", "13345224609.738474920.8088930")
	form.Set("response_url", "https://hooks.slack.test/response")
	body := form.Encode()
	header, _ := signSlackRequest("s3cr3t", body, time.Now())
	req := httptest.NewRequest(http.MethodPost, ManagedSlashPath, strings.NewReader(body))
	req.Header = header
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	rec := httptest.NewRecorder()
	webhook.HandleSlash(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("slash delivery: code=%d, want ACK", rec.Code)
	}
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) && len(stub.cmds) == 0 {
		time.Sleep(10 * time.Millisecond)
	}
	if len(stub.cmds) != 1 {
		t.Fatalf("slash must dispatch detached once, got %d", len(stub.cmds))
	}
	got := stub.cmds[0]
	if got.cmd.Command != "/issue" || got.cmd.TriggerID != "13345224609.738474920.8088930" {
		t.Fatalf("dispatched wrong command: %+v", got.cmd)
	}
	if got.envelopeID != "" {
		t.Fatalf("webhook transport has no socket envelope, got %q", got.envelopeID)
	}
}

func TestManagedWebhook_SlashForgedSignatureRefused(t *testing.T) {
	stub := &stubSlashEnqueuer{}
	webhook, err := NewManagedWebhook(ManagedWebhookConfig{Queries: &fakeAppLookup{}, Slash: stub, SigningSecret: "s3cr3t"})
	if err != nil {
		t.Fatalf("new webhook: %v", err)
	}
	req := httptest.NewRequest(http.MethodPost, ManagedSlashPath, strings.NewReader("command=%2Fissue"))
	req.Header.Set("X-Slack-Request-Timestamp", strconv.FormatInt(time.Now().Unix(), 10))
	req.Header.Set("X-Slack-Signature", "v0=deadbeef")
	rec := httptest.NewRecorder()
	webhook.HandleSlash(rec, req)
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("forged slash: code=%d, want 401", rec.Code)
	}
	if len(stub.cmds) != 0 {
		t.Fatal("forged slash must never dispatch")
	}
}
