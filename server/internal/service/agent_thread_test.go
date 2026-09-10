package service

import (
	"encoding/json"
	"errors"
	"strings"
	"testing"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
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

func TestNormalizeAgentThreadInputAllowsAttachmentOnlyTurn(t *testing.T) {
	content, key, err := normalizeAgentThreadInput(" \n", " attachment-only ", true)
	if err != nil || content != "" || key != "attachment-only" {
		t.Fatalf("attachment-only normalize = %q, %q, %v", content, key, err)
	}

	if _, _, err := normalizeAgentThreadInput(" \n", "text-only"); err == nil {
		t.Fatal("accepted an empty Agent thread turn without attachments")
	}
}

func TestSameUUIDSetIncludesAttachmentIdentity(t *testing.T) {
	id := func(last byte) pgtype.UUID {
		var bytes [16]byte
		bytes[15] = last
		return pgtype.UUID{Bytes: bytes, Valid: true}
	}
	if !sameUUIDSet([]pgtype.UUID{id(1), id(2)}, []pgtype.UUID{id(2), id(1)}) {
		t.Fatal("same attachment IDs in a different order did not match")
	}
	if sameUUIDSet([]pgtype.UUID{id(1)}, []pgtype.UUID{id(2)}) {
		t.Fatal("different attachment IDs matched")
	}
}

func TestAgentThreadMessageExposesOnlyContinuationContent(t *testing.T) {
	task := db.AgentTaskQueue{Context: []byte(`{"agent_thread_message":"next turn","agent_thread_idempotency_key":"secret-receipt"}`)}
	if got := AgentThreadMessage(task); got != "next turn" {
		t.Fatalf("AgentThreadMessage = %q", got)
	}
}

func TestAgentThreadContinuationRecognizesAttachmentOnlyTurn(t *testing.T) {
	var parent pgtype.UUID
	parent.Bytes[0] = 1
	parent.Valid = true
	context, err := json.Marshal(agentThreadContext{ParentTaskID: uuid.UUID(parent.Bytes).String()})
	if err != nil {
		t.Fatal(err)
	}
	task := db.AgentTaskQueue{
		Context:              context,
		TriggerEvidenceKind:  pgtype.Text{String: "agent_thread_continuation", Valid: true},
		TriggerEvidenceRefID: parent,
	}
	if !AgentThreadContinuation(task) {
		t.Fatal("attachment-only continuation was not recognized")
	}
}

func TestAgentThreadAutomationRunIDValidatesContinuationLineage(t *testing.T) {
	id := func(last byte) pgtype.UUID {
		var bytes [16]byte
		bytes[15] = last
		return pgtype.UUID{Bytes: bytes, Valid: true}
	}
	rootID, childID, grandchildID := id(1), id(2), id(3)
	automationRunID, agentID, runtimeID := id(4), id(5), id(6)
	continuation := func(taskID, parentID pgtype.UUID) db.AgentTaskQueue {
		context, err := json.Marshal(map[string]string{
			"agent_thread_parent_task_id": uuid.UUID(parentID.Bytes).String(),
			"agent_thread_message":        "continue",
		})
		if err != nil {
			t.Fatal(err)
		}
		return db.AgentTaskQueue{
			ID: taskID, AgentID: agentID, RuntimeID: runtimeID,
			SessionID:            pgtype.Text{String: "provider-session", Valid: true},
			Context:              context,
			TriggerEvidenceKind:  pgtype.Text{String: "agent_thread_continuation", Valid: true},
			TriggerEvidenceRefID: parentID,
		}
	}
	root := db.AgentTaskQueue{
		ID: rootID, AgentID: agentID, RuntimeID: runtimeID,
		SessionID:       pgtype.Text{String: "provider-session", Valid: true},
		AutomationRunID: automationRunID,
	}
	child := continuation(childID, rootID)
	grandchild := continuation(grandchildID, childID)

	got, ok := AgentThreadAutomationRunID([]db.AgentTaskQueue{root, child, grandchild})
	if !ok || got != automationRunID {
		t.Fatalf("AgentThreadAutomationRunID() = (%v, %v), want (%v, true)", got, ok, automationRunID)
	}

	wrongEvidence := grandchild
	wrongEvidence.TriggerEvidenceRefID = rootID
	if _, ok := AgentThreadAutomationRunID([]db.AgentTaskQueue{root, child, wrongEvidence}); ok {
		t.Fatal("accepted continuation whose evidence does not name its parent")
	}

	rotatedSession := grandchild
	rotatedSession.SessionID.String = "rotated-provider-session"
	if got, ok := AgentThreadAutomationRunID([]db.AgentTaskQueue{root, child, rotatedSession}); !ok || got != automationRunID {
		t.Fatalf("provider session rotation invalidated the Agent thread: (%v, %v)", got, ok)
	}

	foreignAgent := grandchild
	foreignAgent.AgentID = id(7)
	if _, ok := AgentThreadAutomationRunID([]db.AgentTaskQueue{root, child, foreignAgent}); ok {
		t.Fatal("accepted continuation from a different Agent")
	}

	foreignRuntime := grandchild
	foreignRuntime.RuntimeID = id(8)
	if _, ok := AgentThreadAutomationRunID([]db.AgentTaskQueue{root, child, foreignRuntime}); ok {
		t.Fatal("accepted continuation from a different runtime")
	}

	foreignParent := grandchild
	foreignContext, err := json.Marshal(map[string]string{
		"agent_thread_parent_task_id": uuid.UUID(id(9).Bytes).String(),
		"agent_thread_message":        "continue",
	})
	if err != nil {
		t.Fatal(err)
	}
	foreignParent.Context = foreignContext
	foreignParent.TriggerEvidenceRefID = id(9)
	if _, ok := AgentThreadAutomationRunID([]db.AgentTaskQueue{root, child, foreignParent}); ok {
		t.Fatal("accepted continuation with a foreign parent")
	}
}
