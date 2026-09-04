package dingtalk

import (
	"context"
	"testing"
	"time"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
)

func TestDingTalkAuthenticatedDialConfirmsConnection(t *testing.T) {
	conn := newFakeWSConn()
	connector := newTestConnector(conn, func(context.Context, *botCallbackData) error { return nil })
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	confirmed := false
	ctx = channel.WithRuntimeReporter(ctx, func(_ context.Context, observation channel.RuntimeObservation) bool {
		confirmed = observation.State == "healthy"
		cancel()
		return true
	})
	if err := connector.run(ctx); err != nil {
		t.Fatal(err)
	}
	if !confirmed {
		t.Fatal("authenticated Stream ticket handshake was not reported")
	}
}
