package slack

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgtype"
	slackapi "github.com/slack-go/slack"

	"github.com/patchbay-ai/patchbay/server/internal/events"
	"github.com/patchbay-ai/patchbay/server/internal/service"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

type automationCompletionQueries interface {
	GetAutomationRun(context.Context, pgtype.UUID) (db.AutomationRun, error)
	GetAutomation(context.Context, pgtype.UUID) (db.Automation, error)
	GetAutomationTrigger(context.Context, pgtype.UUID) (db.AutomationTrigger, error)
	GetChannelInstallationInWorkspace(context.Context, db.GetChannelInstallationInWorkspaceParams) (db.ChannelInstallation, error)
	ListCommentsForIssue(context.Context, db.ListCommentsForIssueParams) ([]db.Comment, error)
}

type completionSlackAPI interface {
	AddReactionContext(context.Context, string, slackapi.ItemRef) error
	PostMessageContext(context.Context, string, ...slackapi.MsgOption) (string, string, error)
}

type automationCompletionErrorRecorder interface {
	Exec(context.Context, string, ...any) (pgconn.CommandTag, error)
}

// AutomationCompletionReactor adds the trigger-configured reaction to the
// Slack message that started a successfully completed automation run.
type AutomationCompletionReactor struct {
	q        automationCompletionQueries
	recorder automationCompletionErrorRecorder
	decrypt  Decrypter
	log      *slog.Logger
	newAPI   func(credentials) completionSlackAPI
	react    func(context.Context, pgtype.UUID, pgtype.UUID) error
	complete func(context.Context, pgtype.UUID, pgtype.UUID) error
}

func NewAutomationCompletionReactor(q automationCompletionQueries, recorder automationCompletionErrorRecorder, decrypt Decrypter, logger *slog.Logger) *AutomationCompletionReactor {
	if logger == nil {
		logger = slog.Default()
	}
	r := &AutomationCompletionReactor{
		q: q, recorder: recorder, decrypt: decrypt, log: logger,
		newAPI: func(c credentials) completionSlackAPI { return slackapi.New(c.BotToken) },
	}
	r.react = r.reactConfigured
	r.complete = r.completeConfigured
	return r
}

func (r *AutomationCompletionReactor) Register(bus *events.Bus) {
	if bus != nil {
		bus.Subscribe(protocol.EventAutomationRunDone, r.handleEvent)
	}
}

func (r *AutomationCompletionReactor) handleEvent(e events.Event) {
	payload, ok := e.Payload.(map[string]any)
	if !ok || payload["status"] != "completed" {
		return
	}
	runIDText, _ := payload["run_id"].(string)
	workspaceID, workspaceErr := util.ParseUUID(e.WorkspaceID)
	runID, runErr := util.ParseUUID(runIDText)
	if workspaceErr != nil || runErr != nil || !workspaceID.Valid || !runID.Valid {
		r.log.Warn("slack automation completion: invalid event identity",
			"workspace_id", e.WorkspaceID, "run_id", runIDText)
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := r.Complete(ctx, workspaceID, runID); err != nil {
		recordCtx, recordCancel := context.WithTimeout(context.Background(), 3*time.Second)
		r.recordDeliveryError(recordCtx, runID, err)
		recordCancel()
		r.log.Warn("slack automation completion delivery failed",
			"workspace_id", e.WorkspaceID, "run_id", runIDText, "error", err)
	}
}

func (r *AutomationCompletionReactor) Complete(ctx context.Context, workspaceID, runID pgtype.UUID) error {
	if r == nil || r.q == nil || r.complete == nil {
		return errors.New("slack automation completion reactor is not configured")
	}
	return r.complete(ctx, workspaceID, runID)
}

func (r *AutomationCompletionReactor) completeConfigured(ctx context.Context, workspaceID, runID pgtype.UUID) error {
	return errors.Join(
		r.React(ctx, workspaceID, runID),
		r.sendConfiguredSummary(ctx, workspaceID, runID),
	)
}

func (r *AutomationCompletionReactor) React(ctx context.Context, workspaceID, runID pgtype.UUID) error {
	if r == nil || r.q == nil || r.react == nil {
		return errors.New("slack automation completion reactor is not configured")
	}
	return r.react(ctx, workspaceID, runID)
}

func (r *AutomationCompletionReactor) reactConfigured(ctx context.Context, workspaceID, runID pgtype.UUID) error {
	run, err := r.q.GetAutomationRun(ctx, runID)
	if err != nil {
		return fmt.Errorf("load automation run: %w", err)
	}
	if run.Status != "completed" || !run.TriggerID.Valid {
		return nil
	}
	trigger, err := r.q.GetAutomationTrigger(ctx, run.TriggerID)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			// A trigger may be deleted after the run starts. Its completion
			// reaction is historical, not a delivery failure.
			return nil
		}
		return fmt.Errorf("load automation trigger: %w", err)
	}
	if trigger.Provider != "slack" || !trigger.Preset.Valid || trigger.Preset.String != "slack.message" {
		return nil
	}
	if len(trigger.Config) == 0 {
		return nil
	}
	var config struct {
		InstallationID     string `json:"installation_id"`
		Channel            string `json:"channel"`
		CompletionReaction string `json:"completion_reaction"`
	}
	if err := json.Unmarshal(trigger.Config, &config); err != nil {
		return fmt.Errorf("decode trigger config: %w", err)
	}
	reaction := strings.Trim(strings.TrimSpace(config.CompletionReaction), ":")
	if reaction == "" || reaction == "none" {
		return nil
	}
	installationID, err := util.ParseUUID(config.InstallationID)
	if err != nil || !installationID.Valid {
		return errors.New("completion reaction requires a valid Slack installation")
	}

	var envelope struct {
		Event        string `json:"event"`
		EventPayload struct {
			TeamID string `json:"team_id"`
			Event  struct {
				Type    string `json:"type"`
				Channel string `json:"channel"`
				TS      string `json:"ts"`
			} `json:"event"`
		} `json:"eventPayload"`
	}
	if err := json.Unmarshal(run.TriggerPayload, &envelope); err != nil {
		return fmt.Errorf("decode trigger payload: %w", err)
	}
	if envelope.Event != "slack.message" ||
		(envelope.EventPayload.Event.Type != "message" && envelope.EventPayload.Event.Type != "app_mention") {
		return errors.New("completion reaction source is not a Slack message")
	}
	channelID := strings.TrimSpace(envelope.EventPayload.Event.Channel)
	messageTS := strings.TrimSpace(envelope.EventPayload.Event.TS)
	if channelID == "" || messageTS == "" {
		return errors.New("completion reaction source omitted channel or message timestamp")
	}
	if configuredChannel := strings.TrimSpace(config.Channel); configuredChannel != "" && configuredChannel != channelID {
		return errors.New("completion reaction source no longer matches the configured channel")
	}

	inst, err := r.q.GetChannelInstallationInWorkspace(ctx, db.GetChannelInstallationInWorkspaceParams{
		ID: installationID, WorkspaceID: workspaceID, ChannelType: string(TypeSlack),
	})
	if err != nil {
		return fmt.Errorf("load Slack installation: %w", err)
	}
	if inst.Status != "installed" || inst.HostedPausedAt.Valid {
		return errors.New("Slack installation is not active")
	}
	creds, err := decodeCredentials(inst.Config, r.decrypt)
	if err != nil {
		return err
	}
	if teamID := strings.TrimSpace(envelope.EventPayload.TeamID); teamID != "" && creds.TeamID != teamID {
		return errors.New("completion reaction source belongs to another Slack workspace")
	}
	err = r.newAPI(creds).AddReactionContext(ctx, reaction, slackapi.NewRefToMessage(channelID, messageTS))
	if err == nil || isAlreadyReacted(err) {
		return nil
	}
	return fmt.Errorf("slack reactions.add: %w", err)
}

func (r *AutomationCompletionReactor) sendConfiguredSummary(ctx context.Context, workspaceID, runID pgtype.UUID) error {
	run, err := r.q.GetAutomationRun(ctx, runID)
	if err != nil {
		return fmt.Errorf("load automation run for Slack send: %w", err)
	}
	if run.Status != "completed" {
		return nil
	}
	automation, err := r.q.GetAutomation(ctx, run.AutomationID)
	if err != nil {
		return fmt.Errorf("load automation for Slack send: %w", err)
	}
	if len(automation.Tools) == 0 {
		return nil
	}
	tools := service.ParseAutomationTools(automation.Tools)
	if tools.SlackSend == nil || (tools.SlackSend.Enabled != nil && !*tools.SlackSend.Enabled) {
		return nil
	}
	installationID, err := util.ParseUUID(tools.SlackSend.InstallationID)
	if err != nil || !installationID.Valid {
		return errors.New("Slack send tool requires a valid installation")
	}
	channels := uniqueNonEmpty(tools.SlackSend.ChannelIDs)
	if len(channels) == 0 {
		return errors.New("Slack send tool requires at least one channel")
	}
	inst, err := r.q.GetChannelInstallationInWorkspace(ctx, db.GetChannelInstallationInWorkspaceParams{
		ID: installationID, WorkspaceID: workspaceID, ChannelType: string(TypeSlack),
	})
	if err != nil {
		return fmt.Errorf("load Slack send installation: %w", err)
	}
	if inst.Status != "installed" || inst.HostedPausedAt.Valid {
		return errors.New("Slack send installation is not active")
	}
	creds, err := decodeCredentials(inst.Config, r.decrypt)
	if err != nil {
		return err
	}
	summary, err := r.completedRunSummary(ctx, workspaceID, automation, run)
	if err != nil {
		return err
	}
	message := "Automation completed: " + automation.Title
	if summary != "" {
		message += "\n\n" + summary
	}
	api := r.newAPI(creds)
	var errs []error
	for _, channelID := range channels {
		if hasSlackSummaryDelivery(run.Result, channelID) {
			continue
		}
		if _, _, err := api.PostMessageContext(ctx, channelID,
			slackapi.MsgOptionText(message, false), slackapi.MsgOptionDisableLinkUnfurl()); err != nil {
			errs = append(errs, fmt.Errorf("Slack chat.postMessage to %s: %w", channelID, err))
			continue
		}
		if err := r.recordSlackSummaryDelivery(ctx, runID, channelID); err != nil {
			errs = append(errs, fmt.Errorf("record Slack delivery to %s: %w", channelID, err))
		}
	}
	return errors.Join(errs...)
}

func (r *AutomationCompletionReactor) completedRunSummary(ctx context.Context, workspaceID pgtype.UUID, automation db.Automation, run db.AutomationRun) (string, error) {
	var completed protocol.TaskCompletedPayload
	if len(run.Result) > 0 && json.Unmarshal(run.Result, &completed) == nil {
		if output := strings.TrimSpace(completed.Output); output != "" {
			return truncateSlackSummary(output), nil
		}
	}
	if run.IssueID.Valid {
		comments, err := r.q.ListCommentsForIssue(ctx, db.ListCommentsForIssueParams{
			IssueID: run.IssueID, WorkspaceID: workspaceID, Limit: 50,
		})
		if err != nil {
			return "", fmt.Errorf("load completed automation issue comments: %w", err)
		}
		for i := len(comments) - 1; i >= 0; i-- {
			if comments[i].AuthorType == "agent" && strings.TrimSpace(comments[i].Content) != "" {
				return truncateSlackSummary(comments[i].Content), nil
			}
		}
	}
	return "Completed successfully.", nil
}

func hasSlackSummaryDelivery(result []byte, channelID string) bool {
	var state struct {
		Channels []string `json:"slack_summary_deliveries"`
	}
	if json.Unmarshal(result, &state) != nil {
		return false
	}
	for _, deliveredChannel := range state.Channels {
		if deliveredChannel == channelID {
			return true
		}
	}
	return false
}

func (r *AutomationCompletionReactor) recordSlackSummaryDelivery(ctx context.Context, runID pgtype.UUID, channelID string) error {
	if r.recorder == nil {
		return nil
	}
	_, err := r.recorder.Exec(ctx, `
		UPDATE automation_run
		SET result = (
			CASE
				WHEN result IS NULL OR jsonb_typeof(result) = 'null' THEN '{}'::jsonb
				WHEN jsonb_typeof(result) = 'object' THEN result
				ELSE jsonb_build_object('output', result)
			END
		) || jsonb_build_object(
			'slack_summary_deliveries', COALESCE(
				CASE WHEN jsonb_typeof(result->'slack_summary_deliveries') = 'array' THEN result->'slack_summary_deliveries' END,
				'[]'::jsonb
			) || jsonb_build_array($2::text)
		)
		WHERE id = $1
		  AND NOT (
			COALESCE(
				CASE WHEN jsonb_typeof(result->'slack_summary_deliveries') = 'array' THEN result->'slack_summary_deliveries' END,
				'[]'::jsonb
			) @> jsonb_build_array($2::text)
		  )
	`, runID, channelID)
	return err
}

func (r *AutomationCompletionReactor) recordDeliveryError(ctx context.Context, runID pgtype.UUID, cause error) {
	if r.recorder == nil || cause == nil {
		return
	}
	message := truncateSlackSummary(cause.Error())
	_, err := r.recorder.Exec(ctx, `
		UPDATE automation_run
		SET result = (
			CASE
				WHEN result IS NULL OR jsonb_typeof(result) = 'null' THEN '{}'::jsonb
				WHEN jsonb_typeof(result) = 'object' THEN result
				ELSE jsonb_build_object('output', result)
			END
		) || jsonb_build_object(
			'tool_delivery_errors', COALESCE(
				CASE WHEN jsonb_typeof(result->'tool_delivery_errors') = 'array' THEN result->'tool_delivery_errors' END,
				'[]'::jsonb
			) || jsonb_build_array(jsonb_build_object('tool', 'slack', 'message', $2::text))
		)
		WHERE id = $1
	`, runID, message)
	if err != nil {
		r.log.Warn("slack automation completion: record delivery error failed",
			"run_id", util.UUIDToString(runID), "error", err)
	}
}

func uniqueNonEmpty(values []string) []string {
	seen := make(map[string]struct{}, len(values))
	result := make([]string, 0, len(values))
	for _, value := range values {
		value = strings.TrimSpace(value)
		if value == "" {
			continue
		}
		if _, exists := seen[value]; exists {
			continue
		}
		seen[value] = struct{}{}
		result = append(result, value)
	}
	return result
}

func truncateSlackSummary(value string) string {
	const maxRunes = 3000
	runes := []rune(strings.TrimSpace(value))
	if len(runes) <= maxRunes {
		return string(runes)
	}
	return string(runes[:maxRunes-1]) + "…"
}

func isAlreadyReacted(err error) bool {
	var slackErr slackapi.SlackErrorResponse
	return errors.As(err, &slackErr) && slackErr.Err == "already_reacted"
}
