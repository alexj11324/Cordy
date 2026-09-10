package handler

import (
	"net/http"
	"sync"
	"testing"

	"github.com/orvilo-ai/orvilo/server/internal/events"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

func TestReportTaskMessagePublishesStructuredAttribution(t *testing.T) {
	if testHandler == nil {
		t.Skip("database not available")
	}
	taskID := seedBatchTask(t, "message-attribution")
	sharedBus := testHandler.Bus
	testHandler.Bus = events.New()
	t.Cleanup(func() { testHandler.Bus = sharedBus })

	var mu sync.Mutex
	var got *protocol.TaskMessagePayload
	testHandler.Bus.Subscribe(protocol.EventTaskMessage, func(event events.Event) {
		if event.TaskID != taskID {
			return
		}
		if payload, ok := event.Payload.(protocol.TaskMessagePayload); ok {
			mu.Lock()
			copy := payload
			got = &copy
			mu.Unlock()
		}
	})

	testutil.Call(t, testHandler.ReportTaskMessages, batchMessagesRequest(t, taskID, []any{
		map[string]any{
			"seq": 1, "type": "text", "content": "done",
			"sources":   []any{map[string]any{"id": "docs", "url": "https://example.test/docs"}},
			"citations": []any{map[string]any{"source_id": "docs", "start": 0, "end": 4}},
		},
	})).Want(http.StatusOK)

	mu.Lock()
	defer mu.Unlock()
	if got == nil || len(got.Sources) != 1 || got.Sources[0].ID != "docs" || len(got.Citations) != 1 {
		t.Fatalf("published task attribution = %+v", got)
	}
}
