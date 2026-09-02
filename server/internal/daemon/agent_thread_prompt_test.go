package daemon

import (
	"strings"
	"testing"
)

func TestBuildPromptAgentThreadContinuationPrecedesOrdinaryChat(t *testing.T) {
	prompt := BuildPrompt(Task{
		IssueID:            "issue-1",
		ChatSessionID:      "must-not-be-used",
		ChatMessage:        "ordinary chat",
		AgentThreadMessage: "continue the task",
	}, "codex")
	if !strings.Contains(prompt, "continue the task") || strings.Contains(prompt, "ordinary chat") {
		t.Fatalf("unexpected continuation prompt: %s", prompt)
	}
}
