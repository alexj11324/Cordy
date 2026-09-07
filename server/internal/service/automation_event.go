package service

import (
	"encoding/json"
	"errors"
	"regexp"
	"strings"

	"github.com/google/uuid"
)

// NativeTriggerMatch holds inbound event fields used to filter trigger.config.
type NativeTriggerMatch struct {
	Channel             string
	Text                string
	Emoji               string
	Branch              string
	Label               string
	Labels              []string
	Failed              bool
	Repository          string
	ActorLogin          string
	ReviewState         string
	ThreadState         string
	Conclusion          string
	InstallationID      string
	SenderID            string
	SenderAuthenticated bool
	ThreadTS            string
	TeamID              string
	ProjectID           string
	StatusID            string
}

type githubLabel struct {
	Name string `json:"name"`
}

type githubNativePayload struct {
	Action     string `json:"action"`
	Ref        string `json:"ref"`
	Repository struct {
		FullName string `json:"full_name"`
	} `json:"repository"`
	Sender struct {
		Login string `json:"login"`
	} `json:"sender"`
	Pusher struct {
		Name string `json:"name"`
	} `json:"pusher"`
	Installation struct {
		ID int64 `json:"id"`
	} `json:"installation"`
	PullRequest *struct {
		Draft  bool          `json:"draft"`
		Merged bool          `json:"merged"`
		Labels []githubLabel `json:"labels"`
		User   struct {
			Login string `json:"login"`
		} `json:"user"`
		Base struct {
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
		User struct {
			Login string `json:"login"`
		} `json:"user"`
	} `json:"comment"`
	Review *struct {
		State string `json:"state"`
		Body  string `json:"body"`
		User  struct {
			Login string `json:"login"`
		} `json:"user"`
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
		Type     string          `json:"type"`
		SubType  string          `json:"subtype"`
		BotID    string          `json:"bot_id"`
		User     string          `json:"user"`
		Channel  json.RawMessage `json:"channel"`
		Text     string          `json:"text"`
		TS       string          `json:"ts"`
		ThreadTS string          `json:"thread_ts"`
		Reaction string          `json:"reaction"`
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
		case "ready_for_review":
			return "github.pull_request.opened"
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
	match := NativeTriggerMatch{Repository: p.Repository.FullName, ActorLogin: firstNonEmpty(p.Sender.Login, p.Pusher.Name)}
	if p.Action == "resolved" || p.Action == "unresolved" {
		match.ThreadState = p.Action
	}
	if p.PullRequest != nil {
		if p.Action == "opened" || p.Action == "reopened" || p.Action == "ready_for_review" {
			match.ActorLogin = firstNonEmpty(p.PullRequest.User.Login, match.ActorLogin)
		}
		match.Branch = firstNonEmpty(p.PullRequest.Head.Ref, p.PullRequest.Base.Ref)
	}
	if match.Branch == "" && strings.HasPrefix(p.Ref, "refs/heads/") {
		match.Branch = strings.TrimPrefix(p.Ref, "refs/heads/")
	}
	if p.WorkflowRun != nil {
		match.Conclusion = p.WorkflowRun.Conclusion
		match.Branch = firstNonEmpty(match.Branch, p.WorkflowRun.HeadBranch)
		match.Failed = p.WorkflowRun.Conclusion != "" && p.WorkflowRun.Conclusion != "success"
	}
	if p.CheckSuite != nil {
		match.Conclusion = firstNonEmpty(match.Conclusion, p.CheckSuite.Conclusion)
		match.Branch = firstNonEmpty(match.Branch, p.CheckSuite.HeadBranch)
		match.Failed = match.Failed || (p.CheckSuite.Conclusion != "" && p.CheckSuite.Conclusion != "success")
	}
	if p.CheckRun != nil {
		match.Conclusion = firstNonEmpty(match.Conclusion, p.CheckRun.Conclusion)
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
		match.ActorLogin = firstNonEmpty(p.Comment.User.Login, match.ActorLogin)
		match.Text = p.Comment.Body
	}
	if p.Review != nil {
		match.ActorLogin = firstNonEmpty(p.Review.User.Login, match.ActorLogin)
		match.ReviewState = strings.ToLower(p.Review.State)
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
	return ParseSlackNativeEnvelopeForInstallation(body, "")
}

// ParseSlackNativeEnvelopeForInstallation keeps the provider installation in
// the match record. Slack channel and user IDs are scoped to a Slack workspace,
// so a channel picker must also bind the installation that produced the event.
func ParseSlackNativeEnvelopeForInstallation(body []byte, installationID string) (preset, eventID string, match NativeTriggerMatch) {
	var env slackNativeEnvelope
	if err := json.Unmarshal(body, &env); err != nil {
		return "", "", NativeTriggerMatch{}
	}
	if !slackNativeEventIngestable(env.Event.Type, env.Event.SubType, env.Event.BotID, env.Event.User) {
		return "", "", NativeTriggerMatch{}
	}
	preset = MapSlackEventToPreset(env.Event.Type)
	var channelID string
	if env.Event.Type == "channel_created" {
		var channel struct {
			ID string `json:"id"`
		}
		if err := json.Unmarshal(env.Event.Channel, &channel); err != nil {
			return "", "", NativeTriggerMatch{}
		}
		channelID = channel.ID
	} else if len(env.Event.Channel) > 0 {
		if err := json.Unmarshal(env.Event.Channel, &channelID); err != nil {
			return "", "", NativeTriggerMatch{}
		}
	}
	match.Channel = firstNonEmpty(channelID, env.Event.Item.Channel)
	match.InstallationID = strings.TrimSpace(installationID)
	match.SenderID = strings.TrimSpace(env.Event.User)
	match.ThreadTS = strings.TrimSpace(env.Event.ThreadTS)
	eventID = slackNativeDedupeKey(env, match.Channel)
	match.Text = env.Event.Text
	match.Emoji = strings.Trim(env.Event.Reaction, ":")
	return preset, eventID, match
}

// LinearTriggerMatch extracts the stable provider IDs exposed by the Linear
// catalog. Webhook payloads use nested resources today, while older provider
// deliveries may carry the corresponding *Id scalar fields.
func LinearTriggerMatch(body []byte) NativeTriggerMatch {
	var payload struct {
		Data struct {
			TeamID    string `json:"teamId"`
			ProjectID string `json:"projectId"`
			StateID   string `json:"stateId"`
			Team      *struct {
				ID string `json:"id"`
			} `json:"team"`
			Project *struct {
				ID string `json:"id"`
			} `json:"project"`
			State *struct {
				ID string `json:"id"`
			} `json:"state"`
		} `json:"data"`
	}
	_ = json.Unmarshal(body, &payload)
	match := NativeTriggerMatch{
		TeamID:    strings.TrimSpace(payload.Data.TeamID),
		ProjectID: strings.TrimSpace(payload.Data.ProjectID),
		StatusID:  strings.TrimSpace(payload.Data.StateID),
	}
	if payload.Data.Team != nil {
		match.TeamID = firstNonEmpty(payload.Data.Team.ID, match.TeamID)
	}
	if payload.Data.Project != nil {
		match.ProjectID = firstNonEmpty(payload.Data.Project.ID, match.ProjectID)
	}
	if payload.Data.State != nil {
		match.StatusID = firstNonEmpty(payload.Data.State.ID, match.StatusID)
	}
	return match
}

// ValidateAutomationTriggerConfig checks fields that otherwise fail only when
// an event arrives. Keeping this in service makes create and update paths use
// the same validation and prevents a saved trigger from silently matching
// nothing forever.
func ValidateAutomationTriggerConfig(preset string, config []byte) error {
	if len(config) == 0 {
		return nil
	}
	var object map[string]json.RawMessage
	if err := json.Unmarshal(config, &object); err != nil || object == nil {
		return errors.New("config must be a JSON object")
	}
	var cfg struct {
		Channel             string   `json:"channel"`
		Keyword             string   `json:"keyword"`
		Regex               string   `json:"regex"`
		Emoji               string   `json:"emoji"`
		Branch              string   `json:"branch"`
		Label               string   `json:"label"`
		OnFailure           *bool    `json:"on_failure"`
		Repository          string   `json:"repository"`
		Repositories        []string `json:"repositories"`
		AuthorScope         string   `json:"author_scope"`
		AuthorLogins        []string `json:"author_logins"`
		ReviewState         string   `json:"review_state"`
		ThreadState         string   `json:"thread_state"`
		Conclusion          string   `json:"conclusion"`
		InstallationID      string   `json:"installation_id"`
		SenderScope         string   `json:"sender_scope"`
		IgnoreThreadReplies *bool    `json:"ignore_thread_replies"`
		CompletionReaction  string   `json:"completion_reaction"`
		TeamID              string   `json:"team_id"`
		ProjectID           string   `json:"project_id"`
		StatusID            string   `json:"status_id"`
	}
	if err := json.Unmarshal(config, &cfg); err != nil {
		return errors.New("config must be a JSON object")
	}
	if cfg.Channel != "" && preset != "slack.message" && preset != "slack.reaction" {
		return errors.New("channel is only supported for Slack message and reaction triggers")
	}
	if cfg.Emoji != "" && preset != "slack.reaction" {
		return errors.New("emoji is only supported for Slack reaction triggers")
	}
	if cfg.OnFailure != nil && preset != "github.ci_completed" && preset != "github.workflow_run.completed" {
		return errors.New("on_failure is only supported for GitHub CI and workflow completion triggers")
	}
	if regex := strings.TrimSpace(cfg.Regex); regex != "" {
		if _, err := regexp.Compile(regex); err != nil {
			return errors.New("config.regex must be a valid regular expression")
		}
	}
	if preset == "slack.reaction" && strings.TrimSpace(cfg.Keyword) != "" {
		return errors.New("keyword is not supported for Slack reaction triggers")
	}
	if cfg.Repository != "" {
		if !strings.HasPrefix(preset, "github.") {
			return errors.New("repository is only supported for GitHub triggers")
		}
		parts := strings.Split(cfg.Repository, "/")
		if len(parts) != 2 || parts[0] == "" || parts[1] == "" || strings.ContainsAny(cfg.Repository, " :\t\n\r") {
			return errors.New("config.repository must use owner/repository format")
		}
	}
	if cfg.Repositories != nil {
		if !strings.HasPrefix(preset, "github.") {
			return errors.New("repositories is only supported for GitHub triggers")
		}
		if len(cfg.Repositories) == 0 {
			return errors.New("config.repositories must select at least one repository")
		}
		for _, repository := range cfg.Repositories {
			parts := strings.Split(repository, "/")
			if len(parts) != 2 || parts[0] == "" || parts[1] == "" || strings.ContainsAny(repository, " :\t\n\r") {
				return errors.New("config.repositories must use owner/repository values")
			}
		}
	}
	if cfg.AuthorScope != "" {
		if preset != "github.pull_request.opened" && preset != "github.push_to_branch" {
			return errors.New("author_scope is only supported for GitHub pull request opened and push triggers")
		}
		if cfg.AuthorScope != "anyone" && cfg.AuthorScope != "me" && cfg.AuthorScope != "specific" {
			return errors.New("config.author_scope must be anyone, me, or specific")
		}
		if cfg.AuthorScope != "anyone" && len(cfg.AuthorLogins) == 0 {
			return errors.New("config.author_logins is required for the selected author scope")
		}
	}
	if cfg.ReviewState != "" && preset != "github.pull_request.review_submitted" {
		return errors.New("review_state is only supported for GitHub review submitted triggers")
	}
	if cfg.ThreadState != "" && preset != "github.pull_request.review_thread" {
		return errors.New("thread_state is only supported for GitHub review thread triggers")
	}
	if cfg.Conclusion != "" && preset != "github.ci_completed" && preset != "github.workflow_run.completed" {
		return errors.New("conclusion is only supported for GitHub CI and workflow completion triggers")
	}
	if cfg.InstallationID != "" {
		if !strings.HasPrefix(preset, "slack.") {
			return errors.New("installation_id is only supported for Slack triggers")
		}
		if _, err := uuid.Parse(cfg.InstallationID); err != nil {
			return errors.New("config.installation_id must be a UUID")
		}
	}
	if cfg.SenderScope != "" && preset != "slack.message" && preset != "slack.reaction" {
		return errors.New("sender_scope is only supported for Slack message and reaction triggers")
	}
	if cfg.SenderScope != "" && cfg.SenderScope != "anyone" && cfg.SenderScope != "authenticated" {
		return errors.New("config.sender_scope must be anyone or authenticated")
	}
	if cfg.IgnoreThreadReplies != nil && preset != "slack.message" {
		return errors.New("ignore_thread_replies is only supported for Slack message triggers")
	}
	if strings.TrimSpace(cfg.Keyword) != "" && strings.TrimSpace(cfg.Regex) != "" {
		return errors.New("config.keyword and config.regex are mutually exclusive")
	}
	if cfg.CompletionReaction != "" {
		if preset != "slack.message" {
			return errors.New("completion_reaction is only supported for Slack message triggers")
		}
		if !regexp.MustCompile(`^[A-Za-z0-9_+-]+$`).MatchString(strings.Trim(cfg.CompletionReaction, ":")) {
			return errors.New("config.completion_reaction must be a Slack emoji name")
		}
	}
	if cfg.TeamID != "" && !strings.HasPrefix(preset, "linear.") {
		return errors.New("team_id is only supported for Linear triggers")
	}
	if cfg.ProjectID != "" && preset != "linear.issue.created" && preset != "linear.issue.status_changed" {
		return errors.New("project_id is only supported for Linear issue triggers")
	}
	if cfg.StatusID != "" && preset != "linear.issue.status_changed" {
		return errors.New("status_id is only supported for Linear status changed triggers")
	}
	switch cfg.ReviewState {
	case "", "approved", "changes_requested", "commented":
	default:
		return errors.New("config.review_state must be approved, changes_requested, or commented")
	}
	switch cfg.ThreadState {
	case "", "resolved", "unresolved":
	default:
		return errors.New("config.thread_state must be resolved or unresolved")
	}
	switch cfg.Conclusion {
	case "", "success", "failure", "cancelled":
	default:
		return errors.New("config.conclusion must be success, failure, or cancelled")
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

func slackNativeDedupeKey(env slackNativeEnvelope, channel string) string {
	if (env.Event.Type == "message" || env.Event.Type == "app_mention") && channel != "" && env.Event.TS != "" {
		// Slack delivers an app_mention and a message event for the same
		// message. Their outer event_id values differ, while (channel, ts) is
		// the stable identity already used by the channel engine.
		return channel + ":" + env.Event.TS
	}
	// Reactions refer to the original message's timestamp. Its identity must
	// not collapse distinct emoji/users/events on that same message. Slack's
	// event_id is stable across retries and unique across these events.
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
	var object map[string]json.RawMessage
	if err := json.Unmarshal(config, &object); err != nil || object == nil {
		return false
	}
	var cfg struct {
		Channel             string   `json:"channel"`
		Keyword             string   `json:"keyword"`
		Regex               string   `json:"regex"`
		Emoji               string   `json:"emoji"`
		Branch              string   `json:"branch"`
		Label               string   `json:"label"`
		OnFailure           *bool    `json:"on_failure"`
		Repository          string   `json:"repository"`
		Repositories        []string `json:"repositories"`
		AuthorScope         string   `json:"author_scope"`
		AuthorLogins        []string `json:"author_logins"`
		ReviewState         string   `json:"review_state"`
		ThreadState         string   `json:"thread_state"`
		Conclusion          string   `json:"conclusion"`
		InstallationID      string   `json:"installation_id"`
		SenderScope         string   `json:"sender_scope"`
		IgnoreThreadReplies *bool    `json:"ignore_thread_replies"`
		TeamID              string   `json:"team_id"`
		ProjectID           string   `json:"project_id"`
		StatusID            string   `json:"status_id"`
	}
	if err := json.Unmarshal(config, &cfg); err != nil {
		return false
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
	if cfg.Repository != "" && !strings.EqualFold(cfg.Repository, match.Repository) {
		return false
	}
	if len(cfg.Repositories) > 0 {
		matched := false
		for _, repository := range cfg.Repositories {
			if strings.EqualFold(repository, match.Repository) {
				matched = true
				break
			}
		}
		if !matched {
			return false
		}
	}
	if cfg.AuthorScope == "me" || cfg.AuthorScope == "specific" {
		matched := false
		for _, login := range cfg.AuthorLogins {
			if strings.EqualFold(login, match.ActorLogin) {
				matched = true
				break
			}
		}
		if !matched {
			return false
		}
	}
	if cfg.ReviewState != "" && cfg.ReviewState != match.ReviewState {
		return false
	}
	if cfg.ThreadState != "" && cfg.ThreadState != match.ThreadState {
		return false
	}
	if cfg.Conclusion != "" && cfg.Conclusion != match.Conclusion {
		return false
	}
	if cfg.InstallationID != "" && cfg.InstallationID != match.InstallationID {
		return false
	}
	if cfg.SenderScope == "authenticated" && !match.SenderAuthenticated {
		return false
	}
	// Cursor's unfiltered Slack message trigger is top-level only. Selecting a
	// keyword or regex opts into matching replies in threads as documented.
	if match.ThreadTS != "" && (cfg.IgnoreThreadReplies == nil || *cfg.IgnoreThreadReplies) {
		return false
	}
	if cfg.TeamID != "" && cfg.TeamID != match.TeamID {
		return false
	}
	if cfg.ProjectID != "" && cfg.ProjectID != match.ProjectID {
		return false
	}
	if cfg.StatusID != "" && cfg.StatusID != match.StatusID {
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
