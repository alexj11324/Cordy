package service

import (
	"encoding/json"
	"errors"
	"regexp"
	"strings"
)

// NativeTriggerMatch holds inbound event fields used to filter trigger.config.
type NativeTriggerMatch struct {
	Channel string
	Text    string
	Emoji   string
	Branch  string
	Label   string
	Labels  []string
	Failed  bool
}

type githubLabel struct {
	Name string `json:"name"`
}

type githubNativePayload struct {
	Action       string `json:"action"`
	Ref          string `json:"ref"`
	Installation struct {
		ID int64 `json:"id"`
	} `json:"installation"`
	PullRequest *struct {
		Draft  bool          `json:"draft"`
		Merged bool          `json:"merged"`
		Labels []githubLabel `json:"labels"`
		Base   struct {
			Ref string `json:"ref"`
		} `json:"base"`
		Head struct {
			Ref string `json:"ref"`
		} `json:"head"`
	} `json:"pull_request"`
	Issue *struct {
		PullRequest json.RawMessage `json:"pull_request"`
		Labels      []githubLabel   `json:"labels"`
	} `json:"issue"`
	Label *struct {
		Name string `json:"name"`
	} `json:"label"`
	Comment *struct {
		Body string `json:"body"`
	} `json:"comment"`
	Review *struct {
		State string `json:"state"`
		Body  string `json:"body"`
	} `json:"review"`
	WorkflowRun *struct {
		Conclusion string `json:"conclusion"`
		HeadBranch string `json:"head_branch"`
	} `json:"workflow_run"`
	CheckSuite *struct {
		Conclusion string `json:"conclusion"`
		HeadBranch string `json:"head_branch"`
	} `json:"check_suite"`
	CheckRun *struct {
		Conclusion string `json:"conclusion"`
		CheckSuite struct {
			HeadBranch string `json:"head_branch"`
		} `json:"check_suite"`
	} `json:"check_run"`
}

type slackNativeEnvelope struct {
	EventID string `json:"event_id"`
	Event   struct {
		Type     string `json:"type"`
		SubType  string `json:"subtype"`
		BotID    string `json:"bot_id"`
		User     string `json:"user"`
		Channel  string `json:"channel"`
		Text     string `json:"text"`
		TS       string `json:"ts"`
		Reaction string `json:"reaction"`
		Item     struct {
			Channel string `json:"channel"`
			TS      string `json:"ts"`
		} `json:"item"`
		ChannelName string `json:"name"`
	} `json:"event"`
}

type linearNativePayload struct {
	Type        string          `json:"type"`
	Action      string          `json:"action"`
	UpdatedFrom json.RawMessage `json:"updatedFrom"`
	Data        struct {
		CompletedAt *string `json:"completedAt"`
		Status      string  `json:"status"`
		State       struct {
			Name string `json:"name"`
		} `json:"state"`
	} `json:"data"`
}

func GitHubInstallationID(body []byte) int64 {
	var p githubNativePayload
	if err := json.Unmarshal(body, &p); err != nil {
		return 0
	}
	return p.Installation.ID
}

func MapGitHubEventToPreset(event, action string, body []byte) string {
	var p githubNativePayload
	_ = json.Unmarshal(body, &p)
	if action == "" {
		action = p.Action
	}
	switch event {
	case "pull_request":
		switch action {
		case "opened", "reopened":
			if p.PullRequest != nil && p.PullRequest.Draft {
				return "github.draft.opened"
			}
			return "github.pull_request.opened"
		case "synchronize":
			return "github.pull_request.pushed"
		case "closed":
			if p.PullRequest != nil && p.PullRequest.Merged {
				return "github.pull_request.merged"
			}
		case "labeled", "unlabeled":
			return "github.pull_request.label_changed"
		}
	case "push":
		return "github.push_to_branch"
	case "issue_comment":
		if p.Issue != nil && len(p.Issue.PullRequest) > 0 && string(p.Issue.PullRequest) != "null" {
			return "github.pull_request.comment"
		}
		return "github.issue.comment"
	case "issues":
		if action == "labeled" || action == "unlabeled" {
			return "github.issue.label_changed"
		}
	case "pull_request_review_comment":
		return "github.pull_request.review_comment"
	case "pull_request_review":
		if action == "submitted" {
			return "github.pull_request.review_submitted"
		}
	case "pull_request_review_thread":
		return "github.pull_request.review_thread"
	case "workflow_run":
		if action == "completed" {
			return "github.workflow_run.completed"
		}
	case "check_suite", "check_run":
		// A check suite emits one completion after its individual check runs.
		// The catalog has one CI-completed preset, so use the suite event as its
		// sole source; mapping check_run as well launches the same automation
		// once per run and once more for the suite.
		if event == "check_suite" && action == "completed" {
			return "github.ci_completed"
		}
	}
	return ""
}

func GitHubTriggerMatch(body []byte) NativeTriggerMatch {
	var p githubNativePayload
	_ = json.Unmarshal(body, &p)
	match := NativeTriggerMatch{}
	if p.PullRequest != nil {
		match.Branch = firstNonEmpty(p.PullRequest.Head.Ref, p.PullRequest.Base.Ref)
	}
	if match.Branch == "" && strings.HasPrefix(p.Ref, "refs/heads/") {
		match.Branch = strings.TrimPrefix(p.Ref, "refs/heads/")
	}
	if p.WorkflowRun != nil {
		match.Branch = firstNonEmpty(match.Branch, p.WorkflowRun.HeadBranch)
		match.Failed = p.WorkflowRun.Conclusion != "" && p.WorkflowRun.Conclusion != "success"
	}
	if p.CheckSuite != nil {
		match.Branch = firstNonEmpty(match.Branch, p.CheckSuite.HeadBranch)
		match.Failed = match.Failed || (p.CheckSuite.Conclusion != "" && p.CheckSuite.Conclusion != "success")
	}
	if p.CheckRun != nil {
		match.Branch = firstNonEmpty(match.Branch, p.CheckRun.CheckSuite.HeadBranch)
		match.Failed = match.Failed || (p.CheckRun.Conclusion != "" && p.CheckRun.Conclusion != "success")
	}
	if p.Label != nil {
		match.Label = p.Label.Name
		match.Labels = append(match.Labels, p.Label.Name)
	}
	if p.PullRequest != nil {
		for _, label := range p.PullRequest.Labels {
			match.Labels = append(match.Labels, label.Name)
		}
	}
	if p.Issue != nil {
		for _, label := range p.Issue.Labels {
			match.Labels = append(match.Labels, label.Name)
		}
	}
	if p.Comment != nil {
		match.Text = p.Comment.Body
	}
	if p.Review != nil {
		match.Text = firstNonEmpty(match.Text, p.Review.Body)
	}
	return match
}

func MapSlackEventToPreset(innerType string) string {
	switch innerType {
	case "message", "app_mention":
		return "slack.message"
	case "reaction_added", "reaction_removed":
		return "slack.reaction"
	case "channel_created":
		return "slack.channel_created"
	default:
		return ""
	}
}

func ParseSlackNativeEnvelope(body []byte) (preset, eventID string, match NativeTriggerMatch) {
	var env slackNativeEnvelope
	if err := json.Unmarshal(body, &env); err != nil {
		return "", "", NativeTriggerMatch{}
	}
	if !slackNativeEventIngestable(env.Event.Type, env.Event.SubType, env.Event.BotID, env.Event.User) {
		return "", "", NativeTriggerMatch{}
	}
	preset = MapSlackEventToPreset(env.Event.Type)
	eventID = slackNativeDedupeKey(env)
	match.Channel = firstNonEmpty(env.Event.Channel, env.Event.Item.Channel)
	match.Text = env.Event.Text
	match.Emoji = strings.Trim(env.Event.Reaction, ":")
	return preset, eventID, match
}

// ValidateAutomationTriggerConfig checks fields that otherwise fail only when
// an event arrives. Keeping this in service makes create and update paths use
// the same validation and prevents a saved trigger from silently matching
// nothing forever.
func ValidateAutomationTriggerConfig(preset string, config []byte) error {
	if len(config) == 0 {
		return nil
	}
	var cfg struct {
		Keyword string `json:"keyword"`
		Regex   string `json:"regex"`
	}
	if err := json.Unmarshal(config, &cfg); err != nil {
		return errors.New("config must be a JSON object")
	}
	if regex := strings.TrimSpace(cfg.Regex); regex != "" {
		if _, err := regexp.Compile(regex); err != nil {
			return errors.New("config.regex must be a valid regular expression")
		}
	}
	if preset == "slack.reaction" && strings.TrimSpace(cfg.Keyword) != "" {
		return errors.New("keyword is not supported for Slack reaction triggers")
	}
	return nil
}

func slackNativeEventIngestable(eventType, subtype, botID, user string) bool {
	switch eventType {
	case "message":
		return user != "" && botID == "" && isSlackIngestableSubtype(subtype)
	case "app_mention":
		return user != "" && botID == ""
	default:
		return true
	}
}

func isSlackIngestableSubtype(subtype string) bool {
	switch subtype {
	case "", "thread_broadcast", "file_share":
		return true
	default:
		return false
	}
}

func slackNativeDedupeKey(env slackNativeEnvelope) string {
	channel := firstNonEmpty(env.Event.Channel, env.Event.Item.Channel)
	ts := firstNonEmpty(env.Event.TS, env.Event.Item.TS)
	if channel != "" && ts != "" {
		// Slack delivers an app_mention and a message event for the same
		// message. Their outer event_id values differ, while (channel, ts) is
		// the stable identity already used by the channel engine.
		return channel + ":" + ts
	}
	return strings.TrimSpace(env.EventID)
}

func MapLinearEventToPreset(eventType, action string, body []byte) string {
	var p linearNativePayload
	_ = json.Unmarshal(body, &p)
	typ := firstNonEmpty(eventType, p.Type)
	act := firstNonEmpty(action, p.Action)
	typ = strings.TrimSpace(typ)
	if i := strings.Index(typ, ":"); i >= 0 {
		if strings.TrimSpace(act) == "" {
			act = typ[i+1:]
		}
		typ = typ[:i]
	}
	switch strings.ToLower(typ) {
	case "issue":
		if strings.EqualFold(act, "create") {
			return "linear.issue.created"
		}
		if strings.EqualFold(act, "update") && linearStatusChanged(p.UpdatedFrom) {
			return "linear.issue.status_changed"
		}
	case "cycle":
		if strings.EqualFold(act, "update") && linearCycleCompleted(p) {
			return "linear.cycle.ended"
		}
	}
	return ""
}

func linearStatusChanged(updatedFrom json.RawMessage) bool {
	if len(updatedFrom) == 0 {
		return false
	}
	var prev map[string]any
	if err := json.Unmarshal(updatedFrom, &prev); err != nil {
		return false
	}
	_, hasState := prev["stateId"]
	_, hasStatus := prev["status"]
	return hasState || hasStatus
}

func linearCycleCompleted(p linearNativePayload) bool {
	if p.Data.CompletedAt == nil || strings.TrimSpace(*p.Data.CompletedAt) == "" {
		return false
	}
	var previous map[string]json.RawMessage
	if err := json.Unmarshal(p.UpdatedFrom, &previous); err != nil {
		return false
	}
	previousCompletedAt, changed := previous["completedAt"]
	if !changed {
		return false
	}
	return len(previousCompletedAt) == 0 || string(previousCompletedAt) == "null" || string(previousCompletedAt) == `""`
}

func TriggerConfigMatches(config []byte, match NativeTriggerMatch) bool {
	if len(config) == 0 {
		return true
	}
	var cfg struct {
		Channel   string `json:"channel"`
		Keyword   string `json:"keyword"`
		Regex     string `json:"regex"`
		Emoji     string `json:"emoji"`
		Branch    string `json:"branch"`
		Label     string `json:"label"`
		OnFailure *bool  `json:"on_failure"`
	}
	if err := json.Unmarshal(config, &cfg); err != nil {
		return true
	}
	if cfg.Channel != "" && !channelMatches(cfg.Channel, match.Channel) {
		return false
	}
	if cfg.Keyword != "" && !strings.Contains(strings.ToLower(match.Text), strings.ToLower(cfg.Keyword)) {
		return false
	}
	if cfg.Regex != "" {
		re, err := regexp.Compile(cfg.Regex)
		if err != nil || !re.MatchString(match.Text) {
			return false
		}
	}
	if cfg.Emoji != "" && strings.Trim(cfg.Emoji, ":") != strings.Trim(match.Emoji, ":") {
		return false
	}
	if cfg.Branch != "" && !branchMatches(cfg.Branch, match.Branch) {
		return false
	}
	if cfg.Label != "" && !labelMatches(cfg.Label, match) {
		return false
	}
	if cfg.OnFailure != nil && *cfg.OnFailure && !match.Failed {
		return false
	}
	return true
}

func labelMatches(want string, match NativeTriggerMatch) bool {
	if strings.EqualFold(strings.TrimSpace(want), strings.TrimSpace(match.Label)) {
		return true
	}
	for _, label := range match.Labels {
		if strings.EqualFold(strings.TrimSpace(want), strings.TrimSpace(label)) {
			return true
		}
	}
	return false
}

func channelMatches(want, got string) bool {
	want = strings.TrimPrefix(strings.TrimSpace(want), "#")
	got = strings.TrimPrefix(strings.TrimSpace(got), "#")
	return strings.EqualFold(want, got)
}

func branchMatches(want, got string) bool {
	want = strings.TrimPrefix(strings.TrimSpace(want), "refs/heads/")
	got = strings.TrimPrefix(strings.TrimSpace(got), "refs/heads/")
	return want == got
}

func firstNonEmpty(values ...string) string {
	for _, v := range values {
		if strings.TrimSpace(v) != "" {
			return v
		}
	}
	return ""
}
