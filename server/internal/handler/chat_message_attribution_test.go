package handler

import (
	"encoding/json"
	"testing"

	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

func TestChatMessageToResponsePreservesStructuredAttribution(t *testing.T) {
	sources, _ := json.Marshal([]protocol.MessageSource{{ID: "docs", URL: "https://example.test/docs", Title: "Docs"}})
	citations, _ := json.Marshal([]protocol.MessageCitation{{SourceID: "docs", Start: 0, End: 4}})
	response := chatMessageToResponse(db.ChatMessage{
		Role: "assistant", Content: "done [plain](https://example.test/plain)", Sources: sources, Citations: citations,
	}, nil)
	if len(response.Sources) != 1 || response.Sources[0].ID != "docs" || len(response.Citations) != 1 {
		t.Fatalf("response attribution = sources=%#v citations=%#v", response.Sources, response.Citations)
	}
}
