package handler

import (
	"encoding/json"
	"net/http"
	"strings"
	"testing"
)

// ── Token generation ────────────────────────────────────────────────────────

func TestGenerateWebhookToken_PrefixAndLength(t *testing.T) {
	token, err := generateWebhookToken()
	if err != nil {
		t.Fatalf("generateWebhookToken: %v", err)
	}
	if !strings.HasPrefix(token, "awt_") {
		t.Fatalf("expected awt_ prefix, got %q", token)
	}
	// 32 random bytes -> 43 base64-url chars (no padding).
	if len(token) != len("awt_")+43 {
		t.Fatalf("unexpected token length: %d (token=%q)", len(token), token)
	}
}

func TestGenerateWebhookToken_Uniqueness(t *testing.T) {
	seen := make(map[string]struct{}, 128)
	for i := 0; i < 128; i++ {
		token, err := generateWebhookToken()
		if err != nil {
			t.Fatalf("generateWebhookToken: %v", err)
		}
		if _, dup := seen[token]; dup {
			t.Fatalf("duplicate token after %d generations: %q", i, token)
		}
		seen[token] = struct{}{}
	}
}

func TestGenerateWebhookToken_NoUnsafeURLChars(t *testing.T) {
	token, err := generateWebhookToken()
	if err != nil {
		t.Fatalf("generateWebhookToken: %v", err)
	}
	if strings.ContainsAny(token, "+/= ") {
		t.Fatalf("token has unsafe characters: %q", token)
	}
}

// ── Payload normalization ───────────────────────────────────────────────────

func TestNormalizeWebhookPayload_PreservesCallerProvidedEnvelope(t *testing.T) {
	body := []byte(`{"event":"caller.event","eventPayload":{"k":"v"}}`)
	headers := http.Header{}
	headers.Set("Content-Type", "application/json")

	env, err := normalizeWebhookPayload(body, headers)
	if err != nil {
		t.Fatalf("normalize: %v", err)
	}
	if env.Event != "caller.event" {
		t.Fatalf("event: got %q want %q", env.Event, "caller.event")
	}
	var inner map[string]string
	if err := json.Unmarshal(env.EventPayload, &inner); err != nil {
		t.Fatalf("eventPayload not preserved: %v", err)
	}
	if inner["k"] != "v" {
		t.Fatalf("eventPayload contents lost: %#v", inner)
	}
	if env.Request.ContentType != "application/json" {
		t.Fatalf("contentType: %q", env.Request.ContentType)
	}
	if env.Request.ReceivedAt == "" {
		t.Fatal("receivedAt not set")
	}
}

func TestNormalizeWebhookPayload_GitHubHeaderInferEvent(t *testing.T) {
	body := []byte(`{"action":"opened","pull_request":{"number":7}}`)
	headers := http.Header{}
	headers.Set("Content-Type", "application/json")
	headers.Set("X-GitHub-Event", "pull_request")

	env, err := normalizeWebhookPayload(body, headers)
	if err != nil {
		t.Fatalf("normalize: %v", err)
	}
	if env.Event != "github.pull_request.opened" {
		t.Fatalf("github event: got %q", env.Event)
	}
	// Original body preserved in eventPayload.
	if !strings.Contains(string(env.EventPayload), `"pull_request"`) {
		t.Fatalf("body not preserved in eventPayload: %s", env.EventPayload)
	}
}

func TestNormalizeWebhookPayload_GitLabHeader(t *testing.T) {
	body := []byte(`{"object_kind":"push"}`)
	headers := http.Header{}
	headers.Set("X-Gitlab-Event", "Push Hook")

	env, err := normalizeWebhookPayload(body, headers)
	if err != nil {
		t.Fatalf("normalize: %v", err)
	}
	if env.Event != "gitlab.Push Hook" {
		t.Fatalf("gitlab event: got %q", env.Event)
	}
}

func TestNormalizeWebhookPayload_BodyEventField(t *testing.T) {
	body := []byte(`{"event":"demo.received","data":{"x":1}}`)
	headers := http.Header{}

	env, err := normalizeWebhookPayload(body, headers)
	if err != nil {
		t.Fatalf("normalize: %v", err)
	}
	if env.Event != "demo.received" {
		t.Fatalf("event: %q", env.Event)
	}
}

func TestNormalizeWebhookPayload_BodyTypeFallback(t *testing.T) {
	body := []byte(`{"type":"foo.bar"}`)
	env, err := normalizeWebhookPayload(body, http.Header{})
	if err != nil {
		t.Fatalf("normalize: %v", err)
	}
	if env.Event != "foo.bar" {
		t.Fatalf("event: %q", env.Event)
	}
}

func TestNormalizeWebhookPayload_BodyActionFallback(t *testing.T) {
	body := []byte(`{"action":"opened"}`)
	env, err := normalizeWebhookPayload(body, http.Header{})
	if err != nil {
		t.Fatalf("normalize: %v", err)
	}
	if env.Event != "opened" {
		t.Fatalf("event: %q", env.Event)
	}
}

func TestNormalizeWebhookPayload_DefaultEvent(t *testing.T) {
	body := []byte(`{"foo":"bar"}`)
	env, err := normalizeWebhookPayload(body, http.Header{})
	if err != nil {
		t.Fatalf("normalize: %v", err)
	}
	if env.Event != "webhook.received" {
		t.Fatalf("event: %q", env.Event)
	}
	if !strings.Contains(string(env.EventPayload), `"foo"`) {
		t.Fatalf("event payload not preserved: %s", env.EventPayload)
	}
}

func TestNormalizeWebhookPayload_PreservesArray(t *testing.T) {
	body := []byte(`[{"a":1},{"b":2}]`)
	env, err := normalizeWebhookPayload(body, http.Header{})
	if err != nil {
		t.Fatalf("normalize: %v", err)
	}
	if env.Event != "webhook.received" {
		t.Fatalf("array event: %q", env.Event)
	}
	var arr []map[string]int
	if err := json.Unmarshal(env.EventPayload, &arr); err != nil {
		t.Fatalf("array not preserved: %v", err)
	}
	if len(arr) != 2 {
		t.Fatalf("array length: %d", len(arr))
	}
}

func TestNormalizeWebhookPayload_RejectsInvalidJSON(t *testing.T) {
	if _, err := normalizeWebhookPayload([]byte(`not json`), http.Header{}); err == nil {
		t.Fatal("expected error on invalid JSON")
	}
}

func TestNormalizeWebhookPayload_RejectsScalarBody(t *testing.T) {
	// Bare scalar JSON ("hello", 42) is not a useful webhook payload.
	if _, err := normalizeWebhookPayload([]byte(`"hello"`), http.Header{}); err == nil {
		t.Fatal("expected error on scalar JSON body")
	}
}

func TestNormalizeWebhookPayload_GitHubHeaderWithoutAction(t *testing.T) {
	body := []byte(`{"some":"thing"}`)
	headers := http.Header{}
	headers.Set("X-GitHub-Event", "push")
	env, err := normalizeWebhookPayload(body, headers)
	if err != nil {
		t.Fatalf("normalize: %v", err)
	}
	if env.Event != "github.push" {
		t.Fatalf("event: %q", env.Event)
	}
}

func TestNormalizeWebhookPayload_XEventTypeHeader(t *testing.T) {
	body := []byte(`{"a":1}`)
	headers := http.Header{}
	headers.Set("X-Event-Type", "custom.thing")
	env, err := normalizeWebhookPayload(body, headers)
	if err != nil {
		t.Fatalf("normalize: %v", err)
	}
	if env.Event != "custom.thing" {
		t.Fatalf("event: %q", env.Event)
	}
}

// ── Event filter helpers ────────────────────────────────────────────────────

func TestWebhookEventAllowedByTriggerScope_NoFiltersAllowsAll(t *testing.T) {
	if !webhookEventAllowedByTriggerScope(nil, WebhookEnvelope{Event: "github.push"}) {
		t.Fatal("nil filters should allow all")
	}
	if !webhookEventAllowedByTriggerScope([]byte{}, WebhookEnvelope{Event: "github.push"}) {
		t.Fatal("empty filters should allow all")
	}
	if !webhookEventAllowedByTriggerScope([]byte("[]"), WebhookEnvelope{Event: "github.push"}) {
		t.Fatal("empty JSON array should allow all")
	}
}

func TestWebhookEventAllowedByTriggerScope_FiltersUndeclaredEvent(t *testing.T) {
	filters := []byte(`[{"event":"workflow_run","actions":["completed"]}]`)
	env := WebhookEnvelope{Event: "github.push", EventPayload: json.RawMessage(`{"action":"pushed"}`)}
	if webhookEventAllowedByTriggerScope(filters, env) {
		t.Fatal("undeclared event should be filtered")
	}
}

func TestWebhookEventAllowedByTriggerScope_FiltersUndeclaredAction(t *testing.T) {
	filters := []byte(`[{"event":"workflow_run","actions":["completed"]}]`)
	env := WebhookEnvelope{Event: "github.workflow_run.in_progress", EventPayload: json.RawMessage(`{"action":"in_progress"}`)}
	if webhookEventAllowedByTriggerScope(filters, env) {
		t.Fatal("undeclared action should be filtered")
	}
}

func TestWebhookEventAllowedByTriggerScope_AllowsDeclaredAction(t *testing.T) {
	filters := []byte(`[{"event":"workflow_run","actions":["completed"]}]`)
	env := WebhookEnvelope{Event: "github.workflow_run.completed", EventPayload: json.RawMessage(`{"action":"completed"}`)}
	if !webhookEventAllowedByTriggerScope(filters, env) {
		t.Fatal("declared action should be allowed")
	}
}

func TestWebhookEventAllowedByTriggerScope_AnyActionWhenEmpty(t *testing.T) {
	filters := []byte(`[{"event":"workflow_run"}]`)
	env := WebhookEnvelope{Event: "github.workflow_run.in_progress", EventPayload: json.RawMessage(`{"action":"in_progress"}`)}
	if !webhookEventAllowedByTriggerScope(filters, env) {
		t.Fatal("empty actions should allow any action for the event")
	}
}

// TestWebhookEventAllowedByTriggerScope_MultipleFiltersSameEvent pins the
// fix for PR #3231 review: the matcher used to return false as soon as it
// hit the first event-name match whose actions didn't line up, which made
// later filters covering the same event but different actions unreachable
// (order-dependent silent drops). The fix is to keep scanning and only
// short-circuit on a positive match.
func TestWebhookEventAllowedByTriggerScope_MultipleFiltersSameEvent(t *testing.T) {
	filters := []byte(`[
		{"event":"workflow_run","actions":["completed"]},
		{"event":"workflow_run","actions":["requested"]}
	]`)

	completed := WebhookEnvelope{
		Event:        "github.workflow_run.completed",
		EventPayload: json.RawMessage(`{"action":"completed"}`),
	}
	if !webhookEventAllowedByTriggerScope(filters, completed) {
		t.Fatal("workflow_run.completed should match the first filter")
	}

	requested := WebhookEnvelope{
		Event:        "github.workflow_run.requested",
		EventPayload: json.RawMessage(`{"action":"requested"}`),
	}
	if !webhookEventAllowedByTriggerScope(filters, requested) {
		t.Fatal("workflow_run.requested should match the second filter — pre-fix this silently dropped")
	}

	inProgress := WebhookEnvelope{
		Event:        "github.workflow_run.in_progress",
		EventPayload: json.RawMessage(`{"action":"in_progress"}`),
	}
	if webhookEventAllowedByTriggerScope(filters, inProgress) {
		t.Fatal("workflow_run.in_progress is in neither filter and should be filtered out")
	}
}

// TestWebhookEventAllowedByTriggerScope_MalformedDenies pins the
// fail-closed behavior for corrupted rows. Strict write-time validation
// (validateWebhookEventFilters) is the primary defense; this is the
// defense-in-depth check for "what if a malformed row somehow exists".
func TestWebhookEventAllowedByTriggerScope_MalformedDenies(t *testing.T) {
	corrupt := []byte(`{not a json array}`)
	env := WebhookEnvelope{
		Event:        "github.workflow_run.completed",
		EventPayload: json.RawMessage(`{"action":"completed"}`),
	}
	if webhookEventAllowedByTriggerScope(corrupt, env) {
		t.Fatal("malformed event_filters must fail closed (deny), never widen the allowlist")
	}
}

func TestWebhookEventAllowedByTriggerScope_MultipleFilters(t *testing.T) {
	filters := []byte(`[{"event":"workflow_run","actions":["completed"]},{"event":"check_suite","actions":["completed","failure"]}]`)

	allowed1 := WebhookEnvelope{Event: "github.check_suite.completed", EventPayload: json.RawMessage(`{"action":"completed"}`)}
	if !webhookEventAllowedByTriggerScope(filters, allowed1) {
		t.Fatal("check_suite.completed should be allowed")
	}

	allowed2 := WebhookEnvelope{Event: "github.check_suite.failure", EventPayload: json.RawMessage(`{"action":"failure"}`)}
	if !webhookEventAllowedByTriggerScope(filters, allowed2) {
		t.Fatal("check_suite.failure should be allowed")
	}

	filtered := WebhookEnvelope{Event: "github.check_suite.requested", EventPayload: json.RawMessage(`{"action":"requested"}`)}
	if webhookEventAllowedByTriggerScope(filters, filtered) {
		t.Fatal("check_suite.requested should be filtered")
	}
}

func TestSplitWebhookEvent(t *testing.T) {
	tests := []struct {
		input        string
		wantProvider string
		wantName     string
		wantAction   string
	}{
		{"github.workflow_run.completed", "github", "workflow_run", "completed"},
		{"github.push", "github", "push", ""},
		{"gitlab.Merge Request Hook", "gitlab", "Merge Request Hook", ""},
		{"webhook.received", "", "webhook", "received"},
		{"custom", "", "custom", ""},
	}
	for _, tc := range tests {
		p, n, a := splitWebhookEvent(tc.input)
		if p != tc.wantProvider || n != tc.wantName || a != tc.wantAction {
			t.Fatalf("splitWebhookEvent(%q) = (%q, %q, %q), want (%q, %q, %q)",
				tc.input, p, n, a, tc.wantProvider, tc.wantName, tc.wantAction)
		}
	}
}

// ── Provider shapes whose event is not a top-level string ───────────────────
//
// These four providers back shipped automation templates. Each case pairs the
// delivery shape the provider actually sends with the event filter the
// template seeds, so a normalization regression surfaces as a template that
// silently records `event_filtered`.

func TestNormalizeWebhookPayload_SlackNestedEvent(t *testing.T) {
	body := []byte(`{"type":"event_callback","team_id":"T1","event":{"type":"message","text":"it crashed","channel":"C1"}}`)
	env, err := normalizeWebhookPayload(body, http.Header{})
	if err != nil {
		t.Fatalf("normalize: %v", err)
	}
	if env.Event != "slack.message" {
		t.Fatalf("event: got %q, want slack.message", env.Event)
	}
	if !webhookEventAllowedByTriggerScope([]byte(`[{"event":"message"}]`), env) {
		t.Fatal("fix_bugs_reported_in_slack filter should accept a Slack message event")
	}
}

func TestNormalizeWebhookPayload_SlackNestedEventSubtypeIsAction(t *testing.T) {
	body := []byte(`{"type":"event_callback","event":{"type":"message","subtype":"bot_message"}}`)
	env, err := normalizeWebhookPayload(body, http.Header{})
	if err != nil {
		t.Fatalf("normalize: %v", err)
	}
	if env.Event != "slack.message.bot_message" {
		t.Fatalf("event: got %q, want slack.message.bot_message", env.Event)
	}
	// An action-less filter still takes every subtype.
	if !webhookEventAllowedByTriggerScope([]byte(`[{"event":"message"}]`), env) {
		t.Fatal("action-less message filter should accept a subtyped message")
	}
	if webhookEventAllowedByTriggerScope([]byte(`[{"event":"message","actions":["channel_join"]}]`), env) {
		t.Fatal("subtype should be filterable as an action")
	}
}

func TestNormalizeWebhookPayload_SlackUrlVerificationKeepsTopLevelType(t *testing.T) {
	body := []byte(`{"type":"url_verification","challenge":"abc"}`)
	env, err := normalizeWebhookPayload(body, http.Header{})
	if err != nil {
		t.Fatalf("normalize: %v", err)
	}
	if env.Event != "url_verification" {
		t.Fatalf("event: got %q, want url_verification", env.Event)
	}
}

func TestNormalizeWebhookPayload_LinearHeaderAndBody(t *testing.T) {
	body := []byte(`{"action":"create","type":"Issue","data":{"id":"iss_1","title":"Bug"}}`)

	withHeader := http.Header{}
	withHeader.Set("Linear-Event", "Issue")
	env, err := normalizeWebhookPayload(body, withHeader)
	if err != nil {
		t.Fatalf("normalize: %v", err)
	}
	if env.Event != "linear.issue.create" {
		t.Fatalf("event with header: got %q, want linear.issue.create", env.Event)
	}

	// Same payload without the header still resolves: `type` + a Linear
	// action + `data` is enough to claim the envelope.
	env, err = normalizeWebhookPayload(body, http.Header{})
	if err != nil {
		t.Fatalf("normalize: %v", err)
	}
	if env.Event != "linear.issue.create" {
		t.Fatalf("event without header: got %q, want linear.issue.create", env.Event)
	}
	if !webhookEventAllowedByTriggerScope([]byte(`[{"event":"issue","actions":["create"]}]`), env) {
		t.Fatal("triage_linear_issues filter should accept a Linear issue-created event")
	}
	if webhookEventAllowedByTriggerScope([]byte(`[{"event":"issues","actions":["opened"]}]`), env) {
		t.Fatal("GitHub-shaped filter must not silently match a Linear event")
	}
}

func TestNormalizeWebhookPayload_LinearShapeNotClaimedWithoutEnvelope(t *testing.T) {
	// `type` + `action` alone is a common generic shape; without Linear's
	// `data` object we must not relabel it as Linear.
	body := []byte(`{"type":"Order","action":"create"}`)
	env, err := normalizeWebhookPayload(body, http.Header{})
	if err != nil {
		t.Fatalf("normalize: %v", err)
	}
	if env.Event != "Order" {
		t.Fatalf("event: got %q, want Order", env.Event)
	}
}

func TestNormalizeWebhookPayload_PagerDutyNestedEventType(t *testing.T) {
	body := []byte(`{"event":{"id":"01","event_type":"incident.triggered","resource_type":"incident","data":{"id":"PX"}}}`)
	env, err := normalizeWebhookPayload(body, http.Header{})
	if err != nil {
		t.Fatalf("normalize: %v", err)
	}
	if env.Event != "pagerduty.incident.triggered" {
		t.Fatalf("event: got %q, want pagerduty.incident.triggered", env.Event)
	}
	if !webhookEventAllowedByTriggerScope([]byte(`[{"event":"incident"}]`), env) {
		t.Fatal("investigate_pagerduty_incidents filter should accept a triggered incident")
	}
}

func TestNormalizeWebhookPayload_SentryResourceHeader(t *testing.T) {
	for _, tc := range []struct {
		resource string
		want     string
	}{
		{"error", "sentry.error.created"},
		{"issue", "sentry.issue.created"},
	} {
		headers := http.Header{}
		headers.Set("Sentry-Hook-Resource", tc.resource)
		env, err := normalizeWebhookPayload([]byte(`{"action":"created","data":{"issue":{"id":"1"}}}`), headers)
		if err != nil {
			t.Fatalf("normalize %s: %v", tc.resource, err)
		}
		if env.Event != tc.want {
			t.Fatalf("event for %s: got %q, want %q", tc.resource, env.Event, tc.want)
		}
		if !webhookEventAllowedByTriggerScope([]byte(`[{"event":"error"},{"event":"issue"}]`), env) {
			t.Fatalf("investigate_sentry_issues filter should accept a %s delivery", tc.resource)
		}
	}
}

func TestNormalizeWebhookPayload_CallerEnvelopeStillWinsOverProviderShape(t *testing.T) {
	// An explicit `{event, eventPayload}` envelope is the documented escape
	// hatch and must not be reinterpreted by provider inference.
	body := []byte(`{"event":"custom.thing","eventPayload":{"type":"event_callback"}}`)
	env, err := normalizeWebhookPayload(body, http.Header{})
	if err != nil {
		t.Fatalf("normalize: %v", err)
	}
	if env.Event != "custom.thing" {
		t.Fatalf("event: got %q, want custom.thing", env.Event)
	}
}
