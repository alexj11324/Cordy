package execenv

import (
	"strings"
	"testing"
)

func TestAgentThreadWithoutIssueUsesConversationWorkflow(t *testing.T) {
	for _, ctx := range []TaskContextForEnv{
		{IsAgentThreadContinuation: true, AutomationID: "automation-1", AutomationDescription: "ORIGINAL_AUTOMATION_INSTRUCTIONS"},
		{IsAgentThreadContinuation: true, QuickCreatePrompt: "ORIGINAL_QUICK_CREATE_INPUT"},
	} {
		for name, content := range map[string]string{
			"brief":   buildMetaSkillContent("codex", ctx),
			"context": renderIssueContext("codex", ctx),
		} {
			if !strings.Contains(content, "Agent conversation") {
				t.Errorf("%s missing conversation workflow", name)
			}
			for _, unwanted := range []string{"Final results MUST be delivered", "Run exactly one `orvilo issue create`", "# Issue Execution", "ORIGINAL_AUTOMATION_INSTRUCTIONS", "ORIGINAL_QUICK_CREATE_INPUT"} {
				if strings.Contains(content, unwanted) {
					t.Errorf("%s contains old source instruction %q", name, unwanted)
				}
			}
		}
	}
	if got := classifyTask(TaskContextForEnv{IssueID: "issue-1", IsAgentThreadContinuation: true}); got != kindIssue {
		t.Fatalf("issue-bound continuation lost the issue workflow: %v", got)
	}
}
