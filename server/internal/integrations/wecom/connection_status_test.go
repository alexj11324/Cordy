package wecom

import (
	"context"
	"testing"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
)

func TestWecomSubscribeAckConfirmsConnection(t *testing.T) {
	adapter := &wecomChannel{
		installationID: mustTestUUID(t), botID: "fixture-bot", secret: "fixture-secret",
		handler: func(context.Context, channel.InboundMessage) error { return nil },
		dialer: scriptedDialer{conn: &scriptedConn{}}, wsURL: "wss://example.test/ws",
	}
	confirmed := false
	ctx := channel.WithRuntimeReporter(context.Background(), func(_ context.Context, observation channel.RuntimeObservation) bool {
		confirmed = observation.State == "healthy"
		return true
	})
	// The fixture acknowledges subscribe and then returns a read error.
	if err := adapter.Connect(ctx); err == nil {
		t.Fatal("fixture must disconnect after the successful subscribe")
	}
	if !confirmed {
		t.Fatal("WeCom subscribe acknowledgement was not reported")
	}
}
