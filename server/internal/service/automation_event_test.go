package service

import (
	"testing"
)

func TestMapGitHubEventToPreset(t *testing.T) {
	cases := []struct {
		event, action, body, want string
	}{
		{"pull_request", "opened", `{"pull_request":{"draft":false}}`, "github.pull_request.opened"},
		{"pull_request", "opened", `{"pull_request":{"draft":true}}`, "github.draft.opened"},
		{"pull_request", "synchronize", `{}`, "github.pull_request.pushed"},
		{"pull_request", "closed", `{"pull_request":{"merged":true}}`, "github.pull_request.merged"},
		{"pull_request", "labeled", `{}`, "github.pull_request.label_changed"},
		{"push", "", `{}`, "github.push_to_branch"},
		{"issue_comment", "created", `{"issue":{"pull_request":{"url":"https://x"}}}`, "github.pull_request.comment"},
		{"issue_comment", "created", `{"issue":{}}`, "github.issue.comment"},
		{"workflow_run", "completed", `{}`, "github.workflow_run.completed"},
		{"check_suite", "completed", `{}`, "github.ci_completed"},
		{"pull_request_review", "submitted", `{}`, "github.pull_request.review_submitted"},
		{"ping", "", `{}`, ""},
	}
	for _, tc := range cases {
		got := MapGitHubEventToPreset(tc.event, tc.action, []byte(tc.body))
		if got != tc.want {
			t.Errorf("MapGitHubEventToPreset(%q,%q) = %q, want %q", tc.event, tc.action, got, tc.want)
		}
	}
}

func TestMapSlackAndLinearPresets(t *testing.T) {
	if got := MapSlackEventToPreset("message"); got != "slack.message" {
		t.Fatalf("slack message: %q", got)
	}
	if got := MapSlackEventToPreset("reaction_added"); got != "slack.reaction" {
		t.Fatalf("slack reaction: %q", got)
	}
	if got := MapLinearEventToPreset("Issue", "create", nil); got != "linear.issue.created" {
		t.Fatalf("linear create: %q", got)
	}
	if got := MapLinearEventToPreset("Issue", "update", []byte(`{"updatedFrom":{"stateId":"x"}}`)); got != "linear.issue.status_changed" {
		t.Fatalf("linear status: %q", got)
	}
	completed := "2026-01-01T00:00:00Z"
	if got := MapLinearEventToPreset("Cycle", "update", []byte(`{"data":{"completedAt":"`+completed+`"}}`)); got != "linear.cycle.ended" {
		t.Fatalf("linear cycle: %q", got)
	}
}

func TestTriggerConfigMatches(t *testing.T) {
	if !TriggerConfigMatches(nil, NativeTriggerMatch{}) {
		t.Fatal("empty config should match")
	}
	cfg := []byte(`{"channel":"#bugs","keyword":"sev1","on_failure":true}`)
	if TriggerConfigMatches(cfg, NativeTriggerMatch{Channel: "bugs", Text: "sev1 crash", Failed: false}) {
		t.Fatal("on_failure should reject successful events")
	}
	if !TriggerConfigMatches(cfg, NativeTriggerMatch{Channel: "bugs", Text: "sev1 crash", Failed: true}) {
		t.Fatal("expected match")
	}
	if TriggerConfigMatches([]byte(`{"branch":"main"}`), NativeTriggerMatch{Branch: "feat"}) {
		t.Fatal("branch mismatch")
	}
}

func TestLookupAutomationTriggerPreset(t *testing.T) {
	spec, ok := LookupAutomationTriggerPreset("github.pull_request.opened")
	if !ok || spec.Provider != "github" || !spec.Native {
		t.Fatalf("github preset: %+v ok=%v", spec, ok)
	}
	spec, ok = LookupAutomationTriggerPreset("webhook.received")
	if !ok || spec.Native {
		t.Fatalf("generic webhook should not be native: %+v", spec)
	}
	if IsNativeAutomationProvider("generic") {
		t.Fatal("generic is not native")
	}
}
