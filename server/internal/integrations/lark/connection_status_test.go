package lark

import (
	"context"
	"testing"
	"time"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
)

func TestLarkAuthenticatedDialConfirmsConnection(t *testing.T) {
	decoder := FrameDecoderFunc(func([]byte, Installation) (InboundMessage, bool, error) {
		return InboundMessage{}, false, nil
	})
	connector := quietConnector(t, newFakeWSConn(), decoder, time.Hour)
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	confirmed := false
	ctx = channel.WithRuntimeReporter(ctx, func(_ context.Context, observation channel.RuntimeObservation) bool {
		confirmed = observation.State == "healthy"
		cancel()
		return true
	})
	if err := connector.Run(ctx, Installation{}, func(context.Context, InboundMessage) (DispatchResult, error) { return DispatchResult{}, nil }); err != nil {
		t.Fatal(err)
	}
	if !confirmed {
		t.Fatal("authenticated Lark endpoint handshake was not reported")
	}
}
