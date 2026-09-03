package telegram

import (
	"context"
	"net/http"
	"net/http/httptest"
	"sync/atomic"
	"testing"
	"time"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
)

func TestTelegramSuccessfulPollConfirmsConnection(t *testing.T) {
	var calls atomic.Int64
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if calls.Add(1) > 1 {
			<-r.Context().Done()
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"ok":true,"result":[]}`))
	}))
	defer server.Close()
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	confirmed := false
	ctx = channel.WithRuntimeReporter(ctx, func(_ context.Context, observation channel.RuntimeObservation) bool {
		confirmed = observation.State == "healthy"
		cancel()
		return true
	})
	adapter := &telegramChannel{api: newBotAPI(server.URL, "fixture-token", server.Client()), logger: testLogger(),
		handler: func(context.Context, channel.InboundMessage) error { return nil }}
	if err := adapter.Connect(ctx); err != nil {
		t.Fatal(err)
	}
	if !confirmed {
		t.Fatal("successful authenticated empty poll was not reported")
	}
}
