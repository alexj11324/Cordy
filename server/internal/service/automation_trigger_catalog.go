package service

// Automation trigger catalog IDs match packages/core/automations/trigger-catalog.ts.
// Native GitHub/Slack/Linear presets keep kind=webhook but do not mint a public URL.

type AutomationTriggerSpec struct {
	ID       string
	Provider string
	Kind     string
	Native   bool
}

var automationTriggerCatalog = map[string]AutomationTriggerSpec{
	"scheduled":                         {ID: "scheduled", Provider: "generic", Kind: "schedule", Native: false},
	"github.draft.opened":               {ID: "github.draft.opened", Provider: "github", Kind: "webhook", Native: true},
	"github.pull_request.opened":        {ID: "github.pull_request.opened", Provider: "github", Kind: "webhook", Native: true},
	"github.pull_request.pushed":        {ID: "github.pull_request.pushed", Provider: "github", Kind: "webhook", Native: true},
	"github.pull_request.merged":        {ID: "github.pull_request.merged", Provider: "github", Kind: "webhook", Native: true},
	"github.push_to_branch":             {ID: "github.push_to_branch", Provider: "github", Kind: "webhook", Native: true},
	"github.pull_request.comment":       {ID: "github.pull_request.comment", Provider: "github", Kind: "webhook", Native: true},
	"github.pull_request.label_changed": {ID: "github.pull_request.label_changed", Provider: "github", Kind: "webhook", Native: true},
	"github.issue.label_changed":        {ID: "github.issue.label_changed", Provider: "github", Kind: "webhook", Native: true},
	"github.ci_completed":               {ID: "github.ci_completed", Provider: "github", Kind: "webhook", Native: true},
	"github.issue.comment":              {ID: "github.issue.comment", Provider: "github", Kind: "webhook", Native: true},
	"github.pull_request.review_comment": {ID: "github.pull_request.review_comment", Provider: "github", Kind: "webhook", Native: true},
	"github.pull_request.review_submitted": {ID: "github.pull_request.review_submitted", Provider: "github", Kind: "webhook", Native: true},
	"github.pull_request.review_thread": {ID: "github.pull_request.review_thread", Provider: "github", Kind: "webhook", Native: true},
	"github.workflow_run.completed":     {ID: "github.workflow_run.completed", Provider: "github", Kind: "webhook", Native: true},
	"slack.message":                     {ID: "slack.message", Provider: "slack", Kind: "webhook", Native: true},
	"slack.reaction":                    {ID: "slack.reaction", Provider: "slack", Kind: "webhook", Native: true},
	"slack.channel_created":             {ID: "slack.channel_created", Provider: "slack", Kind: "webhook", Native: true},
	"linear.issue.created":              {ID: "linear.issue.created", Provider: "linear", Kind: "webhook", Native: true},
	"linear.issue.status_changed":       {ID: "linear.issue.status_changed", Provider: "linear", Kind: "webhook", Native: true},
	"linear.cycle.ended":                {ID: "linear.cycle.ended", Provider: "linear", Kind: "webhook", Native: true},
	"webhook.received":                  {ID: "webhook.received", Provider: "generic", Kind: "webhook", Native: false},
}

func LookupAutomationTriggerPreset(id string) (AutomationTriggerSpec, bool) {
	spec, ok := automationTriggerCatalog[id]
	return spec, ok
}

func IsNativeAutomationProvider(provider string) bool {
	switch provider {
	case "github", "slack", "linear":
		return true
	default:
		return false
	}
}
