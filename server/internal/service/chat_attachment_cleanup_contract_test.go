package service

import (
	"strings"
	"testing"
)

func TestChatAttachmentCleanupQueriesFenceAndScopeTargets(t *testing.T) {
	for name, query := range map[string]string{
		"chat session":         listChatSessionAttachmentURLsSQL,
		"runtime system chats": listSystemRuntimeChatAttachmentURLsSQL,
	} {
		t.Run(name, func(t *testing.T) {
			for _, required := range []string{"attachment.chat_session_id", "attachment.chat_message_id", "attachment.url"} {
				if !strings.Contains(query, required) {
					t.Fatalf("query missing %q: %s", required, query)
				}
			}
			if strings.Contains(strings.ToUpper(query), "DELETE FROM") {
				t.Fatal("URL collection query must not delete database rows")
			}
		})
	}

	if !strings.Contains(listChatSessionAttachmentURLsSQL, "attachment.workspace_id = $2") {
		t.Fatal("chat-session URL collection is not workspace-scoped")
	}
	if !strings.Contains(listChatSessionAttachmentURLsSQL, "FOR UPDATE") {
		t.Fatal("chat-session URL collection does not fence the session")
	}
	if !strings.Contains(listSystemRuntimeChatAttachmentURLsSQL, "system_agent.kind = 'system'") {
		t.Fatal("runtime URL collection is not limited to system agents")
	}
	if !strings.Contains(lockSystemRuntimeChatSessionsSQL, "FOR UPDATE OF cs") {
		t.Fatal("runtime session fence does not lock target sessions")
	}
	if !strings.Contains(lockSystemRuntimeChatSessionsSQL, "system_agent.kind = 'system'") {
		t.Fatal("runtime session fence is not limited to system agents")
	}
	if !strings.Contains(lockSystemRuntimeAgentsSQL, "FOR UPDATE OF system_agent") {
		t.Fatal("runtime agent fence does not lock system agents")
	}
	if !strings.Contains(validateSystemRuntimeChatSessionsSQL, "system_agent.kind = 'system'") {
		t.Fatal("runtime session validation is not limited to system agents")
	}
}

func TestDeleteSystemAgentIfOrphanedQueryGuardsUnexpectedSessions(t *testing.T) {
	for _, required := range []string{
		"target.kind = 'system'",
		"target.system_key LIKE 'agent_builder:%'",
		"target.workspace_id = $2",
		"NOT EXISTS",
		"session.agent_id = target.id",
	} {
		if !strings.Contains(deleteSystemAgentIfOrphanedSQL, required) {
			t.Fatalf("orphan guard missing %q: %s", required, deleteSystemAgentIfOrphanedSQL)
		}
	}
}
