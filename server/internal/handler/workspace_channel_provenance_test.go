package handler

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/google/uuid"
	"github.com/patchbay-ai/patchbay/server/internal/service"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

const workspaceChannelMessageEvidenceKind = "workspace_channel_message"

func TestWorkspaceChannelMentionPersistsMessageEvidenceAndActor(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database-backed handler test fixture is required")
	}

	ctx := context.Background()
	agentID := createHandlerTestAgent(t, "Channel Provenance Agent "+uuid.NewString(), []byte("[]"))
	channel := createWorkspaceChannelForTest(t, "provenance-"+uuid.NewString())

	req := newRequest("POST", "/api/workspace-channels/"+uuidToString(channel.ID)+"/messages", map[string]any{
		"content": fmt.Sprintf("[@Provenance](mention://agent/%s) please answer", agentID),
	})
	req = withURLParam(req, "id", uuidToString(channel.ID))
	resp := httptest.NewRecorder()
	testHandler.CreateWorkspaceChannelMessage(resp, req)
	if resp.Code != http.StatusCreated {
		t.Fatalf("create mention message status = %d, body = %s", resp.Code, resp.Body.String())
	}

	var message db.WorkspaceChannelMessage
	if err := json.NewDecoder(resp.Body).Decode(&message); err != nil {
		t.Fatalf("decode channel message: %v", err)
	}

	var (
		evidenceKind  string
		evidenceRef   string
		workspaceID   string
		initiatorID   string
		originatorID  string
		accountableID string
		chatSessionID string
	)
	if err := testPool.QueryRow(ctx, `
		SELECT task.trigger_evidence_kind,
		       task.trigger_evidence_ref_id::text,
		       session.workspace_id::text,
		       task.initiator_user_id::text,
		       task.originator_user_id::text,
		       task.accountable_user_id::text,
		       task.chat_session_id::text
		FROM agent_task_queue AS task
		JOIN chat_session AS session ON session.id = task.chat_session_id
		WHERE task.trigger_evidence_kind = $1
		  AND task.trigger_evidence_ref_id = $2
		  AND task.agent_id = $3
		ORDER BY task.created_at DESC
		LIMIT 1
	`, workspaceChannelMessageEvidenceKind, message.ID, agentID).Scan(
		&evidenceKind,
		&evidenceRef,
		&workspaceID,
		&initiatorID,
		&originatorID,
		&accountableID,
		&chatSessionID,
	); err != nil {
		t.Fatalf("load mention task provenance: %v", err)
	}

	if evidenceKind != workspaceChannelMessageEvidenceKind || evidenceRef != uuidToString(message.ID) {
		t.Fatalf("task evidence = (%q, %q), want (%q, %q)", evidenceKind, evidenceRef, workspaceChannelMessageEvidenceKind, uuidToString(message.ID))
	}
	if workspaceID != testWorkspaceID {
		t.Fatalf("task workspace = %q, want %q", workspaceID, testWorkspaceID)
	}
	for name, got := range map[string]string{
		"initiator":   initiatorID,
		"originator":  originatorID,
		"accountable": accountableID,
	} {
		if got != testUserID {
			t.Fatalf("task %s actor = %q, want %q", name, got, testUserID)
		}
	}
	if chatSessionID == "" {
		t.Fatal("mention task did not retain its chat session")
	}

	var sourceWorkspaceID, sourceChannelID, sourceActorType, sourceActorID string
	if err := testPool.QueryRow(ctx, `
		SELECT workspace_id::text, channel_id::text, author_type, author_id::text
		FROM workspace_channel_message
		WHERE id = $1 AND workspace_id = $2
	`, message.ID, testWorkspaceID).Scan(&sourceWorkspaceID, &sourceChannelID, &sourceActorType, &sourceActorID); err != nil {
		t.Fatalf("load source message: %v", err)
	}
	if sourceWorkspaceID != testWorkspaceID || sourceChannelID != uuidToString(channel.ID) || sourceActorType != "member" || sourceActorID != testUserID {
		t.Fatalf("source binding = (%q, %q, %q, %q), want (%q, %q, member, %q)", sourceWorkspaceID, sourceChannelID, sourceActorType, sourceActorID, testWorkspaceID, uuidToString(channel.ID), testUserID)
	}
}

func TestWorkspaceChannelMessageProvenanceRejectsForeignSourceAndRetries(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database-backed handler test fixture is required")
	}

	ctx := context.Background()
	agentID := createHandlerTestAgent(t, "Channel Provenance Retry Agent "+uuid.NewString(), []byte("[]"))
	channel := createWorkspaceChannelForTest(t, "retry-"+uuid.NewString())
	sessionID := createHandlerTestChatSession(t, agentID)

	var messageID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO workspace_channel_message (
			workspace_id, channel_id, author_type, author_id, content
		) VALUES ($1, $2, 'member', $3, $4)
		RETURNING id
	`, testWorkspaceID, channel.ID, testUserID, "retry source message").Scan(&messageID); err != nil {
		t.Fatalf("seed source channel message: %v", err)
	}
	t.Cleanup(func() {
		_, _ = testPool.Exec(context.Background(), `DELETE FROM agent_task_queue WHERE trigger_evidence_ref_id = $1`, messageID)
	})

	session, err := testHandler.Queries.GetChatSession(ctx, parseUUID(sessionID))
	if err != nil {
		t.Fatalf("load chat session: %v", err)
	}
	agent, err := testHandler.Queries.GetAgent(ctx, parseUUID(agentID))
	if err != nil {
		t.Fatalf("load chat agent: %v", err)
	}

	foreignSource := service.WorkspaceChannelMessageSource{
		WorkspaceID: parseUUID(uuid.NewString()),
		ChannelID:   channel.ID,
		MessageID:   parseUUID(messageID),
		ActorType:   "member",
		ActorID:     parseUUID(testUserID),
	}
	if _, err := testHandler.TaskService.SendDirectChatMessageFromWorkspaceChannel(ctx, session, agent, foreignSource, "first attempt"); err == nil {
		t.Fatal("foreign workspace source was accepted")
	} else if !errors.Is(err, service.ErrInvalidWorkspaceChannelMessageSource) {
		t.Fatalf("foreign workspace source error = %v, want ErrInvalidWorkspaceChannelMessageSource", err)
	}

	// Keep the session in the local workspace but present a source whose
	// channel fence does not match the durable message. This reaches the
	// transaction-local source query rather than only the preflight guard.
	mismatchedChannelSource := foreignSource
	mismatchedChannelSource.WorkspaceID = parseUUID(testWorkspaceID)
	mismatchedChannelSource.ChannelID = parseUUID(uuid.NewString())
	if _, err := testHandler.TaskService.SendDirectChatMessageFromWorkspaceChannel(ctx, session, agent, mismatchedChannelSource, "second attempt"); err == nil {
		t.Fatal("mismatched channel source was accepted")
	}

	var failedTaskCount int
	if err := testPool.QueryRow(ctx, `
		SELECT count(*)
		FROM agent_task_queue
		WHERE trigger_evidence_kind = $1 AND trigger_evidence_ref_id = $2
	`, workspaceChannelMessageEvidenceKind, messageID).Scan(&failedTaskCount); err != nil {
		t.Fatalf("count failed-attempt tasks: %v", err)
	}
	if failedTaskCount != 0 {
		t.Fatalf("failed provenance attempt left %d task rows", failedTaskCount)
	}
	var failedChatMessageCount int
	if err := testPool.QueryRow(ctx, `
		SELECT count(*) FROM chat_message WHERE chat_session_id = $1
	`, sessionID).Scan(&failedChatMessageCount); err != nil {
		t.Fatalf("count failed-attempt chat messages: %v", err)
	}
	if failedChatMessageCount != 0 {
		t.Fatalf("failed provenance attempt left %d chat messages", failedChatMessageCount)
	}

	validSource := foreignSource
	validSource.WorkspaceID = parseUUID(testWorkspaceID)
	sent, err := testHandler.TaskService.SendDirectChatMessageFromWorkspaceChannel(ctx, session, agent, validSource, "retry attempt")
	if err != nil {
		t.Fatalf("valid provenance retry: %v", err)
	}
	if uuidToString(sent.Task.TriggerEvidenceRefID) != messageID || sent.Task.TriggerEvidenceKind.String != workspaceChannelMessageEvidenceKind {
		t.Fatalf("retry task evidence = (%q, %q), want (%q, %q)", sent.Task.TriggerEvidenceKind.String, uuidToString(sent.Task.TriggerEvidenceRefID), workspaceChannelMessageEvidenceKind, messageID)
	}
}
