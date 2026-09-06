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
		{"check_run", "completed", `{}`, ""},
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
	if got := MapLinearEventToPreset("Issue:create", "create", nil); got != "linear.issue.created" {
		t.Fatalf("linear colon fallback: %q", got)
	}
	completed := "2026-01-01T00:00:00Z"
	if got := MapLinearEventToPreset("Cycle", "update", []byte(`{"data":{"completedAt":"`+completed+`"},"updatedFrom":{"completedAt":null}}`)); got != "linear.cycle.ended" {
		t.Fatalf("linear cycle: %q", got)
	}
	if got := MapLinearEventToPreset("Cycle", "update", []byte(`{"data":{"completedAt":"`+completed+`"},"updatedFrom":{"name":"renamed"}}`)); got != "" {
		t.Fatalf("linear cycle rename: %q, want no completion event", got)
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
	if !TriggerConfigMatches([]byte(`{"label":"bug"}`), NativeTriggerMatch{Labels: []string{"feature", "Bug"}}) {
		t.Fatal("label collection should match case-insensitively")
	}
}

func TestGitHubTriggerMatchIncludesResourceLabels(t *testing.T) {
	match := GitHubTriggerMatch([]byte(`{"pull_request":{"labels":[{"name":"bug"},{"name":"backend"}]}}`))
	if !TriggerConfigMatches([]byte(`{"label":"backend"}`), match) {
		t.Fatalf("resource label was not available to the trigger matcher: %+v", match)
	}
}

func TestParseSlackNativeEnvelopeFiltersAndDeduplicates(t *testing.T) {
	body := []byte(`{"event_id":"Ev1","event":{"type":"app_mention","user":"U1","channel":"C1","text":"<@UBOT> hi","ts":"1700000000.000100"}}`)
	preset, eventID, match := ParseSlackNativeEnvelope(body)
	if preset != "slack.message" || eventID != "C1:1700000000.000100" || match.Channel != "C1" {
		t.Fatalf("parsed app mention = preset %q id %q match %+v", preset, eventID, match)
	}

	messageBody := []byte(`{"event_id":"Ev2","event":{"type":"message","user":"U1","channel":"C1","text":"hi","ts":"1700000000.000100"}}`)
	_, messageID, _ := ParseSlackNativeEnvelope(messageBody)
	if messageID != eventID {
		t.Fatalf("message/app_mention dedupe ids differ: %q vs %q", messageID, eventID)
	}

	for _, body := range []string{
		`{"event_id":"bot","event":{"type":"message","user":"U1","bot_id":"B1","channel":"C1","ts":"1"}}`,
		`{"event_id":"edit","event":{"type":"message","user":"U1","subtype":"message_changed","channel":"C1","ts":"2"}}`,
	} {
		if preset, _, _ := ParseSlackNativeEnvelope([]byte(body)); preset != "" {
			t.Fatalf("ineligible Slack event mapped to %q: %s", preset, body)
		}
	}
}

func TestValidateAutomationTriggerConfig(t *testing.T) {
	if err := ValidateAutomationTriggerConfig("slack.message", []byte(`{"regex":"["}`)); err == nil {
		t.Fatal("invalid regex should be rejected")
	}
	if err := ValidateAutomationTriggerConfig("slack.reaction", []byte(`{"keyword":"incident"}`)); err == nil {
		t.Fatal("reaction keyword should be rejected")
	}
	if err := ValidateAutomationTriggerConfig("slack.reaction", []byte(`{"emoji":":rotating_light:"}`)); err != nil {
		t.Fatalf("valid reaction config: %v", err)
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
