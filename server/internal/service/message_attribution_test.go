package service

import (
	"context"
	"encoding/json"
	"testing"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/orvilo-ai/orvilo/server/internal/events"
	"github.com/orvilo-ai/orvilo/server/internal/util"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

func attributionJSON(t *testing.T, value any) []byte {
	t.Helper()
	data, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	return data
}

func TestWriteChatCompletionOutcomePersistsTranscriptAttribution(t *testing.T) {
	pool := newResolveOriginatorPool(t)
	ctx := context.Background()
	q := db.New(pool)
	workspaceID, userID, agentID, _ := seedAttributionFixture(t, pool)

	var sessionID, taskID string
	if err := pool.QueryRow(ctx, `
		INSERT INTO chat_session (workspace_id, agent_id, creator_id)
		VALUES ($1, $2, $3) RETURNING id::text`, workspaceID, agentID, userID).Scan(&sessionID); err != nil {
		t.Fatalf("seed chat session: %v", err)
	}
	if err := pool.QueryRow(ctx, `
		INSERT INTO agent_task_queue (agent_id, runtime_id, chat_session_id, status, priority)
		VALUES ($1, (SELECT runtime_id FROM agent WHERE id = $1), $2, 'running', 2)
		RETURNING id::text`, agentID, sessionID).Scan(&taskID); err != nil {
		t.Fatalf("seed task: %v", err)
	}
	t.Cleanup(func() {
		pool.Exec(context.Background(), `DELETE FROM chat_message WHERE chat_session_id = $1`, sessionID)
		pool.Exec(context.Background(), `DELETE FROM agent_task_queue WHERE id = $1`, taskID)
		pool.Exec(context.Background(), `DELETE FROM chat_session WHERE id = $1`, sessionID)
	})
	if _, err := pool.Exec(ctx, `UPDATE agent_task_queue SET chat_input_task_id = id WHERE id = $1`, taskID); err != nil {
		t.Fatalf("stamp input owner: %v", err)
	}
	if _, err := pool.Exec(ctx, `
		INSERT INTO chat_message (chat_session_id, role, content, task_id)
		VALUES ($1, 'user', 'question', $2)`, sessionID, taskID); err != nil {
		t.Fatalf("seed chat input: %v", err)
	}
	if _, err := pool.Exec(ctx, `
		INSERT INTO task_message (task_id, seq, type, content, sources, citations)
		VALUES
		($1, 1, 'text', 'alpha', $2, $3),
		($1, 2, 'text', ' beta', $4, $5)`,
		taskID,
		attributionJSON(t, []protocol.MessageSource{{ID: "a", URL: "https://example.test/a"}}),
		attributionJSON(t, []protocol.MessageCitation{{SourceID: "a", Start: 0, End: 5}}),
		attributionJSON(t, []protocol.MessageSource{{ID: "a", URL: "https://example.test/b"}}),
		attributionJSON(t, []protocol.MessageCitation{{SourceID: "a", Start: 1, End: 5}}),
	); err != nil {
		t.Fatalf("seed task messages: %v", err)
	}

	result, _ := json.Marshal(protocol.TaskCompletedPayload{Output: "alpha beta"})
	task := db.AgentTaskQueue{
		ID: util.MustParseUUID(taskID), ChatSessionID: util.MustParseUUID(sessionID), AgentID: util.MustParseUUID(agentID),
	}
	svc := &TaskService{Queries: q, TxStarter: pool, Bus: events.New()}
	row, err := svc.writeChatCompletionOutcome(ctx, q, task, result)
	if err != nil || row == nil {
		t.Fatalf("writeChatCompletionOutcome row=%+v err=%v", row, err)
	}
	sources, citations := protocol.DecodeMessageAttribution(row.Content, row.Sources, row.Citations)
	if len(sources) != 2 || sources[1].ID != "a#2" || len(citations) != 2 || citations[1].SourceID != "a#2" || citations[1].Start != 6 || citations[1].End != 10 {
		t.Fatalf("persisted chat attribution: sources=%#v citations=%#v", sources, citations)
	}
	var done protocol.ChatDonePayload
	svc.Bus.Subscribe(protocol.EventChatDone, func(event events.Event) {
		done, _ = event.Payload.(protocol.ChatDonePayload)
	})
	svc.broadcastChatDone(ctx, task, row, false)
	if len(done.Sources) != 2 || len(done.Citations) != 2 || done.Citations[1].Start != 6 {
		t.Fatalf("chat:done attribution: %+v", done)
	}
}

func TestTaskMessageAttributionForChatOutputShiftsRanges(t *testing.T) {
	messages := []db.TaskMessage{
		{
			Type: "text", Content: pgtype.Text{String: "alpha", Valid: true},
			Sources:   attributionJSON(t, []protocol.MessageSource{{ID: "a", URL: "https://example.test/a"}}),
			Citations: attributionJSON(t, []protocol.MessageCitation{{SourceID: "a", Start: 0, End: 5}}),
		},
		{
			Type: "text", Content: pgtype.Text{String: " beta", Valid: true},
			Sources:   attributionJSON(t, []protocol.MessageSource{{ID: "a", URL: "https://example.test/b"}}),
			Citations: attributionJSON(t, []protocol.MessageCitation{{SourceID: "a", Start: 1, End: 5}}),
		},
	}
	sources, citations := taskMessageAttributionForChatOutput(messages, "alpha beta")
	if len(sources) != 2 || len(citations) != 2 {
		t.Fatalf("sources=%#v citations=%#v", sources, citations)
	}
	if sources[1].ID != "a#2" || citations[1] != (protocol.MessageCitation{SourceID: "a#2", Start: 6, End: 10}) {
		t.Fatalf("shifted citation = %#v", citations[1])
	}

	sources, citations = taskMessageAttributionForChatOutput(messages, "different final text")
	if len(sources) != 0 || len(citations) != 0 {
		t.Fatalf("mismatched final output retained attribution: sources=%#v citations=%#v", sources, citations)
	}
}
