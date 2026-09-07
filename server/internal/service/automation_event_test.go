package service

import (
	"testing"
)

func TestMapGitHubEventToPreset(t *testing.T) {
	cases := []struct {
		event, action, body, want string
	}{
		{"pull_request", "opened", `{"pull_request":{"draft":false}}`, "github.pull_request.opened"},
		{"pull_request", "ready_for_review", `{"pull_request":{"draft":false}}`, "github.pull_request.opened"},
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
	if TriggerConfigMatches([]byte(`{"channel":[]}`), NativeTriggerMatch{Channel: "C1"}) {
		t.Fatal("malformed config must fail closed")
	}
	if TriggerConfigMatches([]byte(`null`), NativeTriggerMatch{}) {
		t.Fatal("non-object config must fail closed")
	}
}

func TestGitHubTriggerSpecificConditions(t *testing.T) {
	cases := []struct {
		name, body, accepted, rejected string
	}{
		{"review", `{"review":{"state":"approved"}}`, `{"review_state":"approved"}`, `{"review_state":"changes_requested"}`},
		{"thread", `{"action":"resolved"}`, `{"thread_state":"resolved"}`, `{"thread_state":"unresolved"}`},
		{"workflow", `{"workflow_run":{"conclusion":"cancelled"}}`, `{"conclusion":"cancelled"}`, `{"conclusion":"success"}`},
		{"check", `{"check_suite":{"conclusion":"success"}}`, `{"conclusion":"success"}`, `{"conclusion":"failure"}`},
		{"repository", `{"repository":{"full_name":"Acme/App"}}`, `{"repository":"acme/app"}`, `{"repository":"acme/other"}`},
		{"repositories", `{"repository":{"full_name":"Acme/App"}}`, `{"repositories":["acme/other","acme/app"]}`, `{"repositories":["acme/other"]}`},
		{"author", `{"sender":{"login":"octocat"}}`, `{"author_scope":"specific","author_logins":["octocat"]}`, `{"author_scope":"specific","author_logins":["hubot"]}`},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			match := GitHubTriggerMatch([]byte(tc.body))
			if !TriggerConfigMatches([]byte(tc.accepted), match) {
				t.Fatal("expected matching condition to be accepted")
			}
			if TriggerConfigMatches([]byte(tc.rejected), match) {
				t.Fatal("mismatching condition must not start an automation")
			}
		})
	}
}

func TestGitHubOpenedAuthorUsesPullRequestAuthor(t *testing.T) {
	match := GitHubTriggerMatch([]byte(`{"action":"ready_for_review","sender":{"login":"maintainer"},"pull_request":{"user":{"login":"author"}}}`))
	if match.ActorLogin != "author" {
		t.Fatalf("ready-for-review actor = %q, want pull request author", match.ActorLogin)
	}
}

func TestGitHubPushAuthorUsesPusherIdentity(t *testing.T) {
	match := GitHubTriggerMatch([]byte(`{"ref":"refs/heads/main","pusher":{"name":"octocat"}}`))
	if match.ActorLogin != "octocat" {
		t.Fatalf("push actor = %q, want pusher identity", match.ActorLogin)
	}
	if !TriggerConfigMatches([]byte(`{"author_scope":"specific","author_logins":["octocat"],"branch":"main"}`), match) {
		t.Fatal("push author and branch filters did not match")
	}
}

func TestValidateGitHubTriggerConditions(t *testing.T) {
	for _, tc := range []struct{ preset, config string }{
		{"github.pull_request.review_submitted", `{"review_state":"typo"}`},
		{"github.pull_request.review_thread", `{"thread_state":"closed"}`},
		{"github.workflow_run.completed", `{"conclusion":"finished"}`},
		{"github.pull_request.opened", `{"repository":"https://github.com/acme/app"}`},
		{"slack.message", `{"repository":"acme/app"}`},
		{"github.pull_request.opened", `{"review_state":"approved"}`},
		{"github.pull_request.opened", `{"repositories":[]}`},
		{"github.pull_request.opened", `{"author_scope":"specific"}`},
		{"github.pull_request.opened", `{"author_scope":"team","author_logins":["octocat"]}`},
		{"github.pull_request.pushed", `{"author_scope":"specific","author_logins":["octocat"]}`},
		{"github.pull_request.review_submitted", `{"thread_state":"resolved"}`},
		{"slack.message", `{"conclusion":"success"}`},
	} {
		if err := ValidateAutomationTriggerConfig(tc.preset, []byte(tc.config)); err == nil {
			t.Errorf("invalid condition accepted for %s: %s", tc.preset, tc.config)
		}
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

func TestSlackChannelCreatedUsesObjectChannel(t *testing.T) {
	// Slack's channel_created envelope contains a channel object, whereas
	// message events contain a channel ID string.
	body := []byte(`{"event_id":"EvChannel","event":{"type":"channel_created","channel":{"id":"C024BE91L","name":"fun","created":1360782804,"creator":"U024BE7LH"}}}`)
	preset, key, match := ParseSlackNativeEnvelope(body)
	if preset != "slack.channel_created" || key != "EvChannel" || match.Channel != "C024BE91L" {
		t.Fatalf("channel_created = %q, %q, %+v", preset, key, match)
	}
}

func TestSlackDistinctReactionsOnOneMessageDoNotDeduplicate(t *testing.T) {
	first := []byte(`{"event_id":"EvReaction1","event":{"type":"reaction_added","user":"U1","reaction":"eyes","item":{"channel":"C1","ts":"1360782400.498405"}}}`)
	second := []byte(`{"event_id":"EvReaction2","event":{"type":"reaction_added","user":"U2","reaction":"thumbsup","item":{"channel":"C1","ts":"1360782400.498405"}}}`)
	_, firstKey, _ := ParseSlackNativeEnvelope(first)
	_, secondKey, _ := ParseSlackNativeEnvelope(second)
	_, retryKey, _ := ParseSlackNativeEnvelope(first)
	if firstKey == secondKey {
		t.Fatalf("distinct reactions collapsed to %q", firstKey)
	}
	if firstKey != retryKey {
		t.Fatalf("provider retry must retain the original dedupe key: %q != %q", firstKey, retryKey)
	}
}

func TestSlackTriggerConditionsUseProviderIdentities(t *testing.T) {
	body := []byte(`{"event_id":"Ev1","event":{"type":"message","user":"U123","channel":"C123","text":"sev1 incident","ts":"1700000000.1","thread_ts":"1700000000.0"}}`)
	_, _, match := ParseSlackNativeEnvelopeForInstallation(body, "installation-1")
	match.SenderAuthenticated = true
	if !TriggerConfigMatches([]byte(`{"installation_id":"installation-1","channel":"C123","sender_scope":"authenticated","keyword":"incident","ignore_thread_replies":false}`), match) {
		t.Fatalf("identity-backed Slack conditions did not match: %+v", match)
	}
	for _, config := range []string{
		`{"installation_id":"installation-2"}`,
		`{"channel":"C999"}`,
		`{"keyword":"missing","ignore_thread_replies":false}`,
	} {
		if TriggerConfigMatches([]byte(config), match) {
			t.Fatalf("mismatching Slack condition accepted: %s", config)
		}
	}
	match.SenderAuthenticated = false
	if TriggerConfigMatches([]byte(`{"sender_scope":"authenticated","ignore_thread_replies":false}`), match) {
		t.Fatal("unlinked Slack sender matched authenticated sender scope")
	}
}

func TestLinearTriggerConditionsUseCatalogIDs(t *testing.T) {
	issue := []byte(`{"type":"Issue","action":"update","data":{"team":{"id":"team-1"},"project":{"id":"project-1"},"state":{"id":"state-1"}}}`)
	match := LinearTriggerMatch(issue)
	if !TriggerConfigMatches([]byte(`{"team_id":"team-1","project_id":"project-1","status_id":"state-1"}`), match) {
		t.Fatalf("Linear catalog IDs did not match: %+v", match)
	}
	for _, config := range []string{`{"team_id":"team-2"}`, `{"project_id":"project-2"}`, `{"status_id":"state-2"}`} {
		if TriggerConfigMatches([]byte(config), match) {
			t.Fatalf("mismatching Linear condition accepted: %s", config)
		}
	}
}

func TestValidateSlackAndLinearTriggerConditions(t *testing.T) {
	invalid := []struct{ preset, config string }{
		{"slack.message", `{"channel":[]}`},
		{"slack.message", `{"keyword":"incident","regex":"incident.*"}`},
		{"slack.reaction", `{"ignore_thread_replies":false}`},
		{"slack.message", `{"sender_scope":"person"}`},
		{"linear.issue.created", `{"status_id":"state-1"}`},
		{"linear.cycle.ended", `{"project_id":"project-1"}`},
		{"github.pull_request.opened", `{"team_id":"team-1"}`},
	}
	for _, tc := range invalid {
		if err := ValidateAutomationTriggerConfig(tc.preset, []byte(tc.config)); err == nil {
			t.Errorf("invalid condition accepted for %s: %s", tc.preset, tc.config)
		}
	}
	for _, tc := range []struct{ preset, config string }{
		{"slack.message", `{"installation_id":"58f8d452-6b50-45fe-8f7f-10ba915f7704","channel":"C123","sender_scope":"authenticated","ignore_thread_replies":true,"completion_reaction":"white_check_mark"}`},
		{"linear.issue.created", `{"team_id":"team-1","project_id":"project-1"}`},
		{"linear.issue.status_changed", `{"team_id":"team-1","project_id":"project-1","status_id":"state-1"}`},
		{"linear.cycle.ended", `{"team_id":"team-1"}`},
	} {
		if err := ValidateAutomationTriggerConfig(tc.preset, []byte(tc.config)); err != nil {
			t.Errorf("valid condition rejected for %s: %v", tc.preset, err)
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
