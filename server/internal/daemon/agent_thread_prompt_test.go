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

func TestBuildPromptAgentThreadWithoutIssueUsesConversationOutput(t *testing.T) {
	for _, task := range []Task{
		{AgentThreadMessage: "read the stored memory", AutomationID: "automation-1", AutomationDescription: "ORIGINAL_AUTOMATION_INSTRUCTIONS"},
		{AgentThreadMessage: "explain the result", QuickCreatePrompt: "ORIGINAL_QUICK_CREATE_INPUT"},
	} {
		prompt := BuildPrompt(task, "codex")
		if !strings.Contains(prompt, task.AgentThreadMessage) || !strings.Contains(prompt, "Respond in this Agent conversation") {
			t.Fatalf("follow-up lost its member turn or direct output route: %s", prompt)
		}
		for _, unwanted := range []string{"Continue working on issue ", "ORIGINAL_AUTOMATION_INSTRUCTIONS", "ORIGINAL_QUICK_CREATE_INPUT"} {
			if strings.Contains(prompt, unwanted) {
				t.Fatalf("follow-up repeats obsolete source workflow %q: %s", unwanted, prompt)
			}
		}
	}
}
