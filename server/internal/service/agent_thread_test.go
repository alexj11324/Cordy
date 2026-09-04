package service

import (
	"errors"
	"strings"
	"testing"

	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func TestAgentThreadAvailabilityFailsClosed(t *testing.T) {
	task := db.AgentTaskQueue{}
	var unavailable *AgentThreadUnavailableError
	if !errors.As(AgentThreadAvailability(task), &unavailable) || unavailable.Reason != AgentThreadProviderSessionNotEstablished {
		t.Fatalf("missing session reason = %#v", unavailable)
	}
	task.SessionID = pgtype.Text{String: "provider-session", Valid: true}
	if err := AgentThreadAvailability(task); err != nil {
		t.Fatalf("established session unavailable: %v", err)
	}
	task.RetiredSessionID = task.SessionID
	if !errors.As(AgentThreadAvailability(task), &unavailable) || unavailable.Reason != AgentThreadProviderSessionRetired {
		t.Fatalf("retired session reason = %#v", unavailable)
	}
}

func TestNormalizeAgentThreadInput(t *testing.T) {
	content, key, err := normalizeAgentThreadInput("  continue\x00 safely  ", " retry-1 ")
	if err != nil || content != "continue safely" || key != "retry-1" {
		t.Fatalf("normalize = %q, %q, %v", content, key, err)
	}
	long := strings.Repeat("界", maxAgentThreadMessageRunes+1)
	content, _, err = normalizeAgentThreadInput(long, "retry-2")
	if err != nil || len([]rune(content)) != maxAgentThreadMessageRunes {
		t.Fatalf("long content length = %d, err=%v", len([]rune(content)), err)
	}
}

func TestAgentThreadMessageExposesOnlyContinuationContent(t *testing.T) {
	task := db.AgentTaskQueue{Context: []byte(`{"agent_thread_message":"next turn","agent_thread_idempotency_key":"secret-receipt"}`)}
	if got := AgentThreadMessage(task); got != "next turn" {
		t.Fatalf("AgentThreadMessage = %q", got)
	}
}
