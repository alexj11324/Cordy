package slack

import (
	"context"
	"encoding/base64"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgtype"
	slackapi "github.com/slack-go/slack"

	"github.com/orvilo-ai/orvilo/server/internal/events"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

type fakeAutomationCompletionQueries struct {
	run         db.AutomationRun
	automation  db.Automation
	trigger     db.AutomationTrigger
	install     db.ChannelInstallation
	comments    []db.Comment
	err         error
	commentsErr error
}

func (q fakeAutomationCompletionQueries) GetAutomation(context.Context, pgtype.UUID) (db.Automation, error) {
	return q.automation, q.err
}

func (q fakeAutomationCompletionQueries) GetAutomationRun(context.Context, pgtype.UUID) (db.AutomationRun, error) {
	return q.run, q.err
}

func (q fakeAutomationCompletionQueries) GetAutomationTrigger(context.Context, pgtype.UUID) (db.AutomationTrigger, error) {
	return q.trigger, q.err
}

func (q fakeAutomationCompletionQueries) GetChannelInstallationInWorkspace(context.Context, db.GetChannelInstallationInWorkspaceParams) (db.ChannelInstallation, error) {
	return q.install, q.err
}

func (q fakeAutomationCompletionQueries) ListCommentsForIssue(context.Context, db.ListCommentsForIssueParams) ([]db.Comment, error) {
	return q.comments, q.commentsErr
}

type recordedCompletionReaction struct {
	name     string
	item     slackapi.ItemRef
	channels []string
	postErr  error
}

func (r *recordedCompletionReaction) AddReactionContext(_ context.Context, name string, item slackapi.ItemRef) error {
	r.name, r.item = name, item
	return nil
}

func (r *recordedCompletionReaction) PostMessageContext(_ context.Context, channel string, _ ...slackapi.MsgOption) (string, string, error) {
	r.channels = append(r.channels, channel)
	return channel, "1.0", r.postErr
}

type recordedCompletionError struct{ calls int }

func (r *recordedCompletionError) Exec(context.Context, string, ...any) (pgconn.CommandTag, error) {
	r.calls++
	return pgconn.CommandTag{}, nil
}

func TestAutomationCompletionReactorAddsConfiguredReaction(t *testing.T) {
	runID := pgtype.UUID{Bytes: [16]byte{1}, Valid: true}
	triggerID := pgtype.UUID{Bytes: [16]byte{2}, Valid: true}
	workspaceID := pgtype.UUID{Bytes: [16]byte{3}, Valid: true}
	installationID := pgtype.UUID{Bytes: [16]byte{4}, Valid: true}
	config := []byte(`{"team_id":"T1","bot_token_encrypted":"` + base64.StdEncoding.EncodeToString([]byte("xoxb-test")) + `"}`)
	q := fakeAutomationCompletionQueries{
		run:     db.AutomationRun{ID: runID, TriggerID: triggerID, Status: "completed", TriggerPayload: []byte(`{"event":"slack.message","eventPayload":{"team_id":"T1","event":{"type":"message","channel":"C1","ts":"1700000000.1"}}}`)},
		trigger: db.AutomationTrigger{ID: triggerID, Provider: "slack", Preset: pgtype.Text{String: "slack.message", Valid: true}, Config: []byte(`{"installation_id":"00000000-0000-0000-0000-000000000004","channel":"C1","completion_reaction":"white_check_mark"}`)},
		install: db.ChannelInstallation{ID: installationID, WorkspaceID: workspaceID, ChannelType: "slack", Status: "installed", Config: config},
	}
	recorded := &recordedCompletionReaction{}
	reactor := NewAutomationCompletionReactor(q, nil, func(ciphertext []byte) ([]byte, error) { return ciphertext, nil }, nil)
	reactor.newAPI = func(credentials) completionSlackAPI { return recorded }

	if err := reactor.React(context.Background(), workspaceID, runID); err != nil {
		t.Fatalf("React: %v", err)
	}
	if recorded.name != "white_check_mark" || recorded.item.Channel != "C1" || recorded.item.Timestamp != "1700000000.1" {
		t.Fatalf("reaction = name %q item %+v", recorded.name, recorded.item)
	}
}

func TestAutomationCompletionReactorOnlyRunsForSuccessfulCompletion(t *testing.T) {
	called := false
	reactor := NewAutomationCompletionReactor(fakeAutomationCompletionQueries{}, nil, nil, nil)
	reactor.complete = func(context.Context, pgtype.UUID, pgtype.UUID) error { called = true; return nil }
	reactor.handleEvent(events.Event{Type: protocol.EventAutomationRunDone, WorkspaceID: "00000000-0000-0000-0000-000000000003", Payload: map[string]any{
		"run_id": "00000000-0000-0000-0000-000000000001", "status": "failed",
	}})
	if called {
		t.Fatal("failed automation must not receive a completion reaction")
	}
}

func TestAutomationCompletionReactorSurfacesProviderErrors(t *testing.T) {
	runID := pgtype.UUID{Bytes: [16]byte{1}, Valid: true}
	workspaceID := pgtype.UUID{Bytes: [16]byte{3}, Valid: true}
	reactor := NewAutomationCompletionReactor(fakeAutomationCompletionQueries{err: errors.New("db unavailable")}, nil, nil, nil)
	if err := reactor.React(context.Background(), workspaceID, runID); err == nil {
		t.Fatal("expected the lookup error to remain observable")
	}
}

func TestAutomationCompletionReactorHonorsNoEmoji(t *testing.T) {
	runID := pgtype.UUID{Bytes: [16]byte{1}, Valid: true}
	triggerID := pgtype.UUID{Bytes: [16]byte{2}, Valid: true}
	workspaceID := pgtype.UUID{Bytes: [16]byte{3}, Valid: true}
	q := fakeAutomationCompletionQueries{
		run:     db.AutomationRun{ID: runID, TriggerID: triggerID, Status: "completed"},
		trigger: db.AutomationTrigger{ID: triggerID, Provider: "slack", Preset: pgtype.Text{String: "slack.message", Valid: true}, Config: []byte(`{"completion_reaction":"none"}`)},
	}
	called := false
	reactor := NewAutomationCompletionReactor(q, nil, nil, nil)
	reactor.newAPI = func(credentials) completionSlackAPI { called = true; return &recordedCompletionReaction{} }
	if err := reactor.React(context.Background(), workspaceID, runID); err != nil {
		t.Fatal(err)
	}
	if called {
		t.Fatal("No Emoji must not call Slack")
	}
}

func TestAutomationCompletionReactorPostsFinalOutputToConfiguredChannels(t *testing.T) {
	runID := pgtype.UUID{Bytes: [16]byte{1}, Valid: true}
	automationID := pgtype.UUID{Bytes: [16]byte{2}, Valid: true}
	workspaceID := pgtype.UUID{Bytes: [16]byte{3}, Valid: true}
	installationID := pgtype.UUID{Bytes: [16]byte{4}, Valid: true}
	config := []byte(`{"team_id":"T1","bot_token_encrypted":"` + base64.StdEncoding.EncodeToString([]byte("xoxb-test")) + `"}`)
	q := fakeAutomationCompletionQueries{
		run:        db.AutomationRun{ID: runID, AutomationID: automationID, Status: "completed", Result: []byte(`{"output":"Fixed the race and verified the regression test."}`)},
		automation: db.Automation{ID: automationID, Title: "Nightly fixer", Tools: []byte(`{"slack_send":{"enabled":true,"installation_id":"00000000-0000-0000-0000-000000000004","channel_ids":["C1","C2","C1"]}}`)},
		install:    db.ChannelInstallation{ID: installationID, WorkspaceID: workspaceID, ChannelType: "slack", Status: "installed", Config: config},
	}
	var channels []string
	var bodies []string
	provider := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, request *http.Request) {
		if err := request.ParseForm(); err != nil {
			t.Error(err)
		}
		channels = append(channels, request.Form.Get("channel"))
		bodies = append(bodies, request.Form.Get("text"))
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"ok":true,"channel":"C1","ts":"1.0"}`))
	}))
	t.Cleanup(provider.Close)
	reactor := NewAutomationCompletionReactor(q, nil, func(ciphertext []byte) ([]byte, error) { return ciphertext, nil }, nil)
	reactor.newAPI = func(c credentials) completionSlackAPI {
		return slackapi.New(c.BotToken, slackapi.OptionAPIURL(provider.URL+"/"))
	}

	if err := reactor.Complete(context.Background(), workspaceID, runID); err != nil {
		t.Fatal(err)
	}
	if len(channels) != 2 || channels[0] != "C1" || channels[1] != "C2" {
		t.Fatalf("posted channels = %#v", channels)
	}
	for _, body := range bodies {
		if body != "Automation completed: Nightly fixer\n\nFixed the race and verified the regression test." {
			t.Fatalf("posted summary = %q", body)
		}
	}
}

func TestAutomationCompletionReactorRecordsSlackDeliveryFailure(t *testing.T) {
	runID := mustUUID(t, "11111111-1111-1111-1111-111111111111")
	automationID := mustUUID(t, "22222222-2222-2222-2222-222222222222")
	workspaceID := mustUUID(t, "33333333-3333-3333-3333-333333333333")
	installationID := mustUUID(t, "44444444-4444-4444-4444-444444444444")
	q := fakeAutomationCompletionQueries{
		run:        db.AutomationRun{ID: runID, AutomationID: automationID, Status: "completed", Result: []byte(`{"output":"Done"}`)},
		automation: db.Automation{ID: automationID, Title: "Notifier", Tools: []byte(`{"slack_send":{"enabled":true,"installation_id":"44444444-4444-4444-4444-444444444444","channel_ids":["C1"]}}`)},
		install:    db.ChannelInstallation{ID: installationID, WorkspaceID: workspaceID, ChannelType: "slack", Status: "installed", Config: []byte(`{"team_id":"T1","bot_token_encrypted":"eG94Yi10ZXN0"}`)},
	}
	recorder := &recordedCompletionError{}
	reactor := NewAutomationCompletionReactor(q, recorder, nil, nil)
	reactor.newAPI = func(credentials) completionSlackAPI {
		return &recordedCompletionReaction{postErr: errors.New("channel_not_found")}
	}
	reactor.handleEvent(events.Event{Type: protocol.EventAutomationRunDone,
		WorkspaceID: "33333333-3333-3333-3333-333333333333",
		Payload:     map[string]any{"run_id": "11111111-1111-1111-1111-111111111111", "status": "completed"},
	})
	if recorder.calls != 1 {
		t.Fatalf("delivery error record calls = %d", recorder.calls)
	}
}

func TestAutomationCompletionReactorSurfacesIssueSummaryLookupFailure(t *testing.T) {
	runID := mustUUID(t, "11111111-1111-1111-1111-111111111111")
	automationID := mustUUID(t, "22222222-2222-2222-2222-222222222222")
	workspaceID := mustUUID(t, "33333333-3333-3333-3333-333333333333")
	installationID := mustUUID(t, "44444444-4444-4444-4444-444444444444")
	q := fakeAutomationCompletionQueries{
		run:         db.AutomationRun{ID: runID, AutomationID: automationID, IssueID: mustUUID(t, "55555555-5555-5555-5555-555555555555"), Status: "completed"},
		automation:  db.Automation{ID: automationID, Title: "Issue notifier", Tools: []byte(`{"slack_send":{"enabled":true,"installation_id":"44444444-4444-4444-4444-444444444444","channel_ids":["C1"]}}`)},
		install:     db.ChannelInstallation{ID: installationID, WorkspaceID: workspaceID, ChannelType: "slack", Status: "installed", Config: []byte(`{"team_id":"T1","bot_token_encrypted":"eG94Yi10ZXN0"}`)},
		commentsErr: errors.New("comments unavailable"),
	}
	reactor := NewAutomationCompletionReactor(q, nil, nil, nil)
	reactor.newAPI = func(credentials) completionSlackAPI { return &recordedCompletionReaction{} }
	if err := reactor.Complete(context.Background(), workspaceID, runID); err == nil || !strings.Contains(err.Error(), "comments unavailable") {
		t.Fatalf("summary lookup error = %v", err)
	}
}
