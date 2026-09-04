package daemon

import (
	"strings"
	"testing"
)

// TestBuildPrompt_HandoffNote_ExecutorBranch verifies a handoff note on an
// issue-executor task renders through the executor branch — it appears in
// the prompt, framed as a handoff (not a comment to reply to), and does not
// trip the quick-create branch.
func TestBuildPrompt_HandoffNote_ExecutorBranch(t *testing.T) {
	note := "Only touch the login flow; do not change payments."
	out := BuildPrompt(Task{IssueID: "issue-123", HandoffNote: note}, "claude")

	if !strings.Contains(out, note) {
		t.Fatalf("handoff note missing from prompt:\n%s", out)
	}
	if !strings.Contains(out, "handoff note") {
		t.Fatalf("expected handoff framing in prompt:\n%s", out)
	}
	if strings.Contains(out, "quick-create assistant") {
		t.Fatalf("handoff task must not use the quick-create prompt branch:\n%s", out)
	}
	// Still an executor task: should point the agent at `patchbay issue get`.
	if !strings.Contains(out, "patchbay issue get issue-123") {
		t.Fatalf("expected executor prompt body:\n%s", out)
	}
}

// TestBuildPrompt_NoHandoffNote_Unchanged verifies the executor prompt is
// unchanged when no handoff note is present (no stray handoff framing).
func TestBuildPrompt_NoHandoffNote_Unchanged(t *testing.T) {
	out := BuildPrompt(Task{IssueID: "issue-123"}, "claude")
	if strings.Contains(out, "handoff note") {
		t.Fatalf("unexpected handoff framing when no note set:\n%s", out)
	}
}
