package metrics

import (
	"regexp"
	"strings"

	"github.com/patchbay-ai/patchbay/server/pkg/taskfailure"
)

const (
	labelSource         = "source"
	labelRuntimeMode    = "runtime_mode"
	labelProvider       = "provider"
	labelTerminalStatus = "terminal_status"
	labelFailureReason  = "failure_reason"
	labelTokenType      = "token_type"
	labelModel          = "model"
	labelModelAlias     = "model_alias"

	// PR3 labels (funnel / community / commercial).
	labelSignupSource = "signup_source"
	labelPlatform     = "platform"
	labelPath         = "path"
	labelCadence      = "cadence"
	labelTriggerKind  = "trigger_kind"
	labelReason       = "reason"
	labelRecoverable  = "recoverable"
	labelKind         = "kind"
	labelStatus       = "status"
	labelEventKind    = "event_kind"
	labelAction       = "action"
	labelResult       = "result"
	labelQuery        = "query"
	labelOp           = "op"
	labelGate         = "gate"
	labelOutcome      = "outcome"
	labelStage        = "stage"
)

var businessMetricLabels = map[string][]string{
	"patchbay_agent_task_enqueued_total":                {labelSource, labelRuntimeMode},
	"patchbay_agent_task_dispatched_total":              {labelSource, labelRuntimeMode},
	"patchbay_agent_task_started_total":                 {labelSource, labelRuntimeMode, labelProvider},
	"patchbay_agent_task_terminal_total":                {labelSource, labelRuntimeMode, labelTerminalStatus},
	"patchbay_agent_task_failed_total":                  {labelSource, labelRuntimeMode, labelFailureReason},
	"patchbay_agent_task_queue_wait_seconds":            {labelSource, labelRuntimeMode},
	"patchbay_agent_task_run_seconds":                   {labelSource, labelRuntimeMode, labelTerminalStatus},
	"patchbay_agent_task_total_seconds":                 {labelSource, labelRuntimeMode, labelTerminalStatus},
	"patchbay_agent_task_in_progress":                   {labelSource, labelRuntimeMode},
	"patchbay_agent_task_iteration_count":               {labelSource, labelTerminalStatus},
	"patchbay_llm_tokens_total":                         {labelProvider, labelModel, labelTokenType, labelRuntimeMode, labelSource},
	"patchbay_llm_cost_usd_total":                       {labelProvider, labelModel, labelTokenType, labelRuntimeMode, labelSource},
	"patchbay_llm_unpriced_tokens_total":                {labelProvider, labelModelAlias, labelTokenType},
	"patchbay_llm_request_total":                        {labelProvider, labelModel, labelRuntimeMode},
	"patchbay_task_queued_expired_total":                {labelSource, labelRuntimeMode},
	"patchbay_task_lease_expired_total":                 {labelSource},
	"patchbay_chat_claim_session_fallback_needed_total": {},
	"patchbay_chat_claim_session_fallback_result_total": {labelResult},
	"patchbay_chat_claim_resume_query_duration_seconds": {labelQuery},
	"patchbay_runtime_sweeper_stage_duration_seconds":   {labelStage},
	"patchbay_runtime_sweeper_candidate_rows_total":     {labelStage},
	"patchbay_runtime_sweeper_rows_changed_total":       {labelStage},
	"patchbay_agent_runtime_lookup_total":               {labelSource, labelResult},

	// PR3 funnel / community / commercial.
	"patchbay_signup_total":                             {labelSignupSource},
	"patchbay_workspace_created_total":                  {labelSource},
	"patchbay_team_invite_sent_total":                   {},
	"patchbay_team_invite_accepted_total":               {},
	"patchbay_onboarding_started_total":                 {labelPlatform},
	"patchbay_onboarding_questionnaire_submitted_total": {},
	"patchbay_onboarding_source_submitted_total":        {},
	"patchbay_onboarding_completed_total":               {labelPath},
	"patchbay_cloud_waitlist_joined_total":              {},
	"patchbay_issue_created_total":                      {labelSource, labelPlatform},
	"patchbay_chat_message_sent_total":                  {labelPlatform},
	"patchbay_agent_created_total":                      {labelRuntimeMode, labelSource},
	"patchbay_team_created_total":                      {},
	"patchbay_automation_created_total":                  {labelCadence},
	"patchbay_issue_executed_total":                     {labelSource},
	"patchbay_runtime_registered_total":                 {labelRuntimeMode, labelProvider},
	"patchbay_runtime_ready_total":                      {labelRuntimeMode, labelProvider},
	"patchbay_runtime_ready_seconds":                    {labelRuntimeMode, labelProvider},
	"patchbay_runtime_failed_total":                     {labelRuntimeMode, labelProvider, labelFailureReason, labelRecoverable},
	"patchbay_runtime_offline_total":                    {labelRuntimeMode, labelProvider},
	"patchbay_runtime_gc_skipped_total":                 {labelReason},
	"patchbay_daemon_ws_message_received_total":         {labelKind},
	"patchbay_automation_run_started_total":              {labelCadence, labelTriggerKind},
	"patchbay_automation_run_terminal_total":             {labelCadence, labelTriggerKind, labelTerminalStatus},
	"patchbay_automation_run_skipped_total":              {labelCadence, labelReason},
	"patchbay_webhook_delivery_total":                   {labelProvider, labelStatus},
	"patchbay_webhook_rate_limited_total":               {labelGate},
	"patchbay_email_rate_limited_total":                 {labelAction, labelGate},
	"patchbay_github_event_received_total":              {labelEventKind, labelAction},
	"patchbay_github_pr_review_total":                   {labelResult},
	"patchbay_cloudruntime_request_total":               {labelOp, labelStatus},
	"patchbay_cloudruntime_request_duration_seconds":    {labelOp},
	"patchbay_feedback_submitted_total":                 {labelKind, labelPlatform},
	"patchbay_contact_sales_submitted_total":            {labelSource},
	"patchbay_chat_output_local_path_total":             {labelKind},
	"patchbay_entitlement_cache_total":                  {labelOutcome},
	"patchbay_entitlement_refresh_total":                {labelOutcome},
	"patchbay_entitlement_refresh_duration_seconds":     {labelOutcome},
	"patchbay_entitlement_decision_total":               {labelGate, labelAction, labelReason},
	"patchbay_entitlement_version_regression_total":     {},
	"patchbay_automation_quota_decision_total":           {labelAction, labelSource, labelResult},
}

var forbiddenMetricLabels = map[string]struct{}{
	"workspace_id": {},
	// installation_id is the same class as the rest: one series per channel
	// installation, growing with tenants rather than with the deployment. It
	// is also the natural thing to reach for in any channel metric — every
	// adapter call site already carries one — which is what makes leaving it
	// off this list a matter of time rather than of luck.
	"installation_id": {},
	"user_id":         {},
	"agent_id":        {},
	"task_id":         {},
	"issue_id":        {},
	"runtime_id":      {},
	"session_id":      {},
	"ip":              {},
}

var (
	knownSources = map[string]string{
		"issue":           "issue",
		"chat":            "chat",
		"automation":       "automation",
		"automation_issue": "automation_issue",
		"quick_create":    "quick_create",
		"manual":          "manual",
		"api":             "api",
		"other":           "other",
	}
	knownRuntimeModes = map[string]string{
		"local":   "local",
		"cloud":   "cloud",
		"unknown": "unknown",
	}
	knownRuntimeProviders = map[string]string{
		"antigravity":   "antigravity",
		"claude":        "claude",
		"codearts":      "codearts",
		"codebuddy":     "codebuddy",
		"codex":         "codex",
		"copilot":       "copilot",
		"cursor":        "cursor",
		"dsh":           "dsh",
		"gemini":        "gemini",
		"grok":          "grok",
		"hermes":        "hermes",
		"kiro":          "kiro",
		"kimi":          "kimi",
		"reasonix":      "reasonix",
		"dim":           "dim",
		"mcode":         "mcode",
		"zeroclaw":      "zeroclaw",
		"patchbay_agent": "patchbay_agent",
		"openclaw":      "openclaw",
		"opencode":      "opencode",
		"deveco":        "deveco",
		"pi":            "pi",
		"qoder":         "qoder",
		"qoderclicn":    "qoderclicn",
		"qwen":          "qwen",
		"traecli":       "traecli",
		"other":         "other",
	}
	knownTerminalStatuses = map[string]string{
		"completed": "completed",
		"failed":    "failed",
		"cancelled": "cancelled",
		"blocked":   "blocked",
		"other":     "other",
	}
	knownTokenTypes = map[string]string{
		"input":       "input",
		"output":      "output",
		"cache_read":  "cache_read",
		"cache_write": "cache_write",
	}
	knownFailureReasons       = map[string]string{}
	knownRuntimeLookupSources = map[string]string{}
	knownRuntimeLookupResults = map[string]string{}
	modelAliasUnsafeRe        = regexp.MustCompile(`[^a-z0-9._:/+-]+`)
)

func init() {
	for _, reason := range taskfailure.AllReasons() {
		knownFailureReasons[reason.String()] = reason.String()
	}
	for _, source := range AllRuntimeLookupSources() {
		knownRuntimeLookupSources[source] = source
	}
	for _, result := range AllRuntimeLookupResults() {
		knownRuntimeLookupResults[result] = result
	}
}

func validateBusinessMetricLabels() {
	for metric, labels := range businessMetricLabels {
		for _, label := range labels {
			if _, forbidden := forbiddenMetricLabels[label]; forbidden {
				panic("forbidden high-cardinality label " + label + " on " + metric)
			}
		}
	}
}

func metricLabels(metric string) []string {
	labels, ok := businessMetricLabels[metric]
	if !ok {
		panic("missing business metric label definition for " + metric)
	}
	return labels
}

// Agent-runtime lookup sources (MUL-6884). A single-row read of agent_runtime
// is issued from a dozen unrelated product paths, and every one of them
// collapses into the same normalized SQL fingerprint in pg_stat_statements —
// so the database can say how many reads happened but never which feature
// asked for them. These labels are that missing half.
//
// The list is a closed enum on purpose: it is the label set of a counter that
// fires on the hottest read in the system, so it must stay bounded no matter
// how many runtimes, workspaces, or routes exist. Anything unrecognized lands
// on RuntimeLookupSourceOther, which should sit at ~0 — a non-zero rate there
// means a new call site was added without classifying it.
const (
	// RuntimeLookupSourceHeartbeatWS is the daemon WebSocket heartbeat, one
	// read per runtime per HeartbeatInterval.
	RuntimeLookupSourceHeartbeatWS = "heartbeat_ws"
	// RuntimeLookupSourceHeartbeatHTTP is the POST /api/daemon/heartbeat
	// fallback used when the WebSocket ack does not arrive.
	RuntimeLookupSourceHeartbeatHTTP = "heartbeat_http"
	// RuntimeLookupSourceDaemonAPI covers daemon-authenticated endpoints other
	// than the heartbeat: WS upgrade, result reports, deregister, task calls.
	RuntimeLookupSourceDaemonAPI = "daemon_api"
	// RuntimeLookupSourceRuntimeModelPoll is the browser polling a model
	// discovery request (every 500ms while the picker waits).
	RuntimeLookupSourceRuntimeModelPoll = "runtime_model_poll"
	// RuntimeLookupSourceRuntimeLocalSkillPoll is the browser polling a local
	// skill / MCP discovery request.
	RuntimeLookupSourceRuntimeLocalSkillPoll = "runtime_local_skill_poll"
	// RuntimeLookupSourceRuntimeLocalSkillImportPoll is the browser polling a
	// local skill import, which the UI allows up to ten of concurrently.
	RuntimeLookupSourceRuntimeLocalSkillImportPoll = "runtime_local_skill_import_poll"
	// RuntimeLookupSourceRuntimeUpdatePoll is the browser polling CLI update
	// progress.
	RuntimeLookupSourceRuntimeUpdatePoll = "runtime_update_poll"
	// RuntimeLookupSourceRuntimeAPI is every other runtime-scoped API call:
	// reads, management, and the read-access gate itself.
	RuntimeLookupSourceRuntimeAPI = "runtime_api"
	// RuntimeLookupSourceIssue covers issue create / assign / quick-create and
	// the trigger-preview readiness checks.
	RuntimeLookupSourceIssue = "issue"
	// RuntimeLookupSourceComment covers mention and sub-issue readiness checks
	// on comments.
	RuntimeLookupSourceComment = "comment"
	// RuntimeLookupSourceChat covers the pre-send readiness check on chat.
	RuntimeLookupSourceChat = "chat"
	// RuntimeLookupSourceAutomation covers automation admission and dispatch.
	RuntimeLookupSourceAutomation = "automation"
	// RuntimeLookupSourceSourceContext covers the source-context quick-create
	// capability gates, including the deliberate post-copy recheck.
	RuntimeLookupSourceSourceContext = "source_context"
	// RuntimeLookupSourceTask covers task analytics context and the usage
	// provider backfill.
	RuntimeLookupSourceTask = "task"
	// RuntimeLookupSourceOther is the catch-all for an unclassified call site.
	RuntimeLookupSourceOther = "other"
)

// Agent-runtime lookup results. Kept separate from the generic status labels
// because the distinction that matters here is "the row is gone" (the daemon
// self-heal signal) versus "the database failed", and collapsing those two
// would hide a real outage behind a normal deletion.
const (
	RuntimeLookupResultOK       = "ok"
	RuntimeLookupResultNotFound = "not_found"
	RuntimeLookupResultError    = "error"
)

// AllRuntimeLookupSources lists every source label in a stable order. Used to
// prewarm the counter so a source that has not fired yet is a visible zero
// instead of an absent series.
func AllRuntimeLookupSources() []string {
	return []string{
		RuntimeLookupSourceHeartbeatWS,
		RuntimeLookupSourceHeartbeatHTTP,
		RuntimeLookupSourceDaemonAPI,
		RuntimeLookupSourceRuntimeModelPoll,
		RuntimeLookupSourceRuntimeLocalSkillPoll,
		RuntimeLookupSourceRuntimeLocalSkillImportPoll,
		RuntimeLookupSourceRuntimeUpdatePoll,
		RuntimeLookupSourceRuntimeAPI,
		RuntimeLookupSourceIssue,
		RuntimeLookupSourceComment,
		RuntimeLookupSourceChat,
		RuntimeLookupSourceAutomation,
		RuntimeLookupSourceSourceContext,
		RuntimeLookupSourceTask,
		RuntimeLookupSourceOther,
	}
}

// AllRuntimeLookupResults lists every result label in a stable order.
func AllRuntimeLookupResults() []string {
	return []string{RuntimeLookupResultOK, RuntimeLookupResultNotFound, RuntimeLookupResultError}
}

// NormalizeAgentRuntimeLookupSource maps a call-site source onto the closed
// enum above. An unknown value becomes "other" rather than a new series.
func NormalizeAgentRuntimeLookupSource(value string) string {
	value = strings.ToLower(strings.TrimSpace(value))
	if normalized, ok := knownRuntimeLookupSources[value]; ok {
		return normalized
	}
	return RuntimeLookupSourceOther
}

// NormalizeAgentRuntimeLookupResult maps a lookup outcome onto the closed
// enum above. An unknown value is treated as an error: a result nobody
// classified is not evidence that the read succeeded.
func NormalizeAgentRuntimeLookupResult(value string) string {
	value = strings.ToLower(strings.TrimSpace(value))
	if normalized, ok := knownRuntimeLookupResults[value]; ok {
		return normalized
	}
	return RuntimeLookupResultError
}

func NormalizeTaskSource(value string) string {
	value = strings.ToLower(strings.TrimSpace(value))
	if normalized, ok := knownSources[value]; ok {
		return normalized
	}
	return "other"
}

func NormalizeRuntimeMode(value string) string {
	value = strings.ToLower(strings.TrimSpace(value))
	if normalized, ok := knownRuntimeModes[value]; ok {
		return normalized
	}
	return "unknown"
}

func NormalizeRuntimeProvider(value string) string {
	value = strings.ToLower(strings.TrimSpace(value))
	if normalized, ok := knownRuntimeProviders[value]; ok {
		return normalized
	}
	return "other"
}

func NormalizeTerminalStatus(value string) string {
	value = strings.ToLower(strings.TrimSpace(value))
	if normalized, ok := knownTerminalStatuses[value]; ok {
		return normalized
	}
	return "other"
}

func NormalizeFailureReason(value string) string {
	value = strings.TrimSpace(value)
	if normalized, ok := knownFailureReasons[value]; ok {
		return normalized
	}
	return taskfailure.Classify(value).String()
}

func NormalizeTokenType(value string) string {
	value = strings.ToLower(strings.TrimSpace(value))
	if normalized, ok := knownTokenTypes[value]; ok {
		return normalized
	}
	return "input"
}

func NormalizeModelAlias(value string) string {
	value = strings.ToLower(strings.TrimSpace(value))
	if value == "" {
		return "unknown"
	}
	value = modelAliasUnsafeRe.ReplaceAllString(value, "_")
	if len(value) > 128 {
		return value[:128]
	}
	return value
}
