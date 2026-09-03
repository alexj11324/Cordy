package slack

import (
	"context"
	"log/slog"
	"testing"

	"github.com/slack-go/slack/socketmode"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
)

func TestSlackConnectionRequiresHello(t *testing.T) {
	var observations []channel.RuntimeObservation
	ctx := channel.WithRuntimeReporter(context.Background(), func(_ context.Context, observation channel.RuntimeObservation) bool {
		observations = append(observations, observation)
		return true
	})
	adapter := &slackChannel{logger: slog.Default()}
	for _, event := range []socketmode.EventType{socketmode.EventTypeConnecting, socketmode.EventTypeConnected} {
		if err := adapter.handleSocketEvent(ctx, nil, socketmode.Event{Type: event}, nil); err != nil {
			t.Fatal(err)
		}
	}
	for _, observation := range observations {
		if observation.State == "healthy" {
			t.Fatal("socket ownership/dial was mistaken for Slack hello")
		}
	}
	if err := adapter.handleSocketEvent(ctx, nil, socketmode.Event{Type: socketmode.EventTypeHello}, nil); err != nil {
		t.Fatal(err)
	}
	if len(observations) == 0 || observations[len(observations)-1].State != "healthy" {
		t.Fatal("Slack hello did not confirm the connection")
	}
	if err := adapter.handleSocketEvent(ctx, nil, socketmode.Event{Type: socketmode.EventTypeIncomingError, Data: "credential-sentinel"}, nil); err != nil {
		t.Fatal(err)
	}
	last := observations[len(observations)-1]
	if last.State != "degraded" || last.ErrorSummary != "" {
		t.Fatalf("socket error did not expose a safe failure state: %+v", last)
	}
}
