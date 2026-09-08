package service

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strings"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/orvilo-ai/orvilo/server/internal/util"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

// AutomationTriggerReadiness is the server-owned explanation for why one
// persisted trigger can or cannot fire. Reasons are intentionally concrete so
// the settings page can show the missing connection or required field beside
// the row; the admission gate remains authoritative for every execution path.
type AutomationTriggerReadiness struct {
	TriggerID string
	Ready     bool
	Reasons   []string
}

// AutomationReadiness is the single trigger configuration admission result.
// An automation with no triggers is deliberately not runnable: manual RunNow,
// schedules, native events, and generic webhooks all share this decision.
type AutomationReadiness struct {
	Ready    bool
	Reasons  []string
	Triggers []AutomationTriggerReadiness
}

type automationTriggerReadinessConfig struct {
	InstallationID string `json:"installation_id"`
	Channel        string `json:"channel"`
}

// CheckAutomationReadiness validates persisted trigger configuration and the
// workspace-owned provider connections needed to execute it. It does not
// mutate legacy rows: an old enabled=false trigger remains visible and blocks
// execution until the user deletes/recreates it or an explicit future
// migration changes its state.
func (s *AutomationService) CheckAutomationReadiness(ctx context.Context, automation db.Automation) (AutomationReadiness, error) {
	triggers, err := s.Queries.ListAutomationTriggers(ctx, automation.ID)
	if err != nil {
		return AutomationReadiness{}, fmt.Errorf("list automation triggers: %w", err)
	}
	result := AutomationReadiness{Ready: true, Triggers: make([]AutomationTriggerReadiness, 0, len(triggers))}
	if len(triggers) == 0 {
		result.Ready = false
		result.Reasons = []string{"Add at least one trigger before running this automation."}
		return result, nil
	}

	githubChecked := false
	githubConnected := false
	linearChecked := false
	linearConnected := false
	for _, trigger := range triggers {
		reasons, err := s.checkAutomationTrigger(ctx, automation.WorkspaceID, trigger, &githubChecked, &githubConnected, &linearChecked, &linearConnected)
		if err != nil {
			return AutomationReadiness{}, fmt.Errorf("check trigger %s readiness: %w", util.UUIDToString(trigger.ID), err)
		}
		item := AutomationTriggerReadiness{
			TriggerID: util.UUIDToString(trigger.ID),
			Ready:     len(reasons) == 0,
			Reasons:   reasons,
		}
		result.Triggers = append(result.Triggers, item)
		if len(reasons) > 0 {
			result.Ready = false
			for _, reason := range reasons {
				result.Reasons = append(result.Reasons, fmt.Sprintf("Trigger %s: %s", util.UUIDToString(trigger.ID), reason))
			}
		}
	}
	return result, nil
}

func (s *AutomationService) checkAutomationTrigger(
	ctx context.Context,
	workspaceID pgtype.UUID,
	trigger db.AutomationTrigger,
	githubChecked, githubConnected, linearChecked, linearConnected *bool,
) ([]string, error) {
	var reasons []string
	if !trigger.Enabled {
		reasons = append(reasons, "This legacy trigger is disabled; delete and add it again to use it.")
	}

	preset := strings.TrimSpace(trigger.Preset.String)
	if trigger.Kind == "schedule" {
		if !trigger.CronExpression.Valid || strings.TrimSpace(trigger.CronExpression.String) == "" {
			reasons = append(reasons, "Schedule expression is missing.")
		}
		if !trigger.NextRunAt.Valid {
			reasons = append(reasons, "Schedule has no valid next run.")
		}
		return reasons, nil
	}
	if trigger.Kind != "webhook" {
		return append(reasons, "Trigger type is no longer supported."), nil
	}
	provider := strings.TrimSpace(trigger.Provider)
	if !trigger.Preset.Valid || preset == "" {
		// Rows created before the preset catalog existed are still valid generic
		// webhooks when their bearer token is present. Do not turn a supported
		// legacy ingress URL into a dead automation merely because it has no
		// catalog id.
		if provider == "" || provider == "generic" {
			if !trigger.WebhookToken.Valid || strings.TrimSpace(trigger.WebhookToken.String) == "" {
				reasons = append(reasons, "Webhook URL is missing.")
			}
			return reasons, nil
		}
		return append(reasons, "Choose a trigger event."), nil
	}
	spec, ok := LookupAutomationTriggerPreset(preset)
	if !ok {
		return append(reasons, "Trigger event is no longer available."), nil
	}
	if provider == "" {
		provider = spec.Provider
	}
	if provider != spec.Provider {
		reasons = append(reasons, "Trigger provider does not match its event.")
	}
	if err := ValidateAutomationTriggerConfig(preset, trigger.Config); err != nil {
		reasons = append(reasons, err.Error())
	}

	switch provider {
	case "generic":
		if !trigger.WebhookToken.Valid || strings.TrimSpace(trigger.WebhookToken.String) == "" {
			reasons = append(reasons, "Webhook URL is missing.")
		}
	case "github":
		if !*githubChecked {
			*githubChecked = true
			installations, err := s.Queries.ListGitHubInstallationsByWorkspace(ctx, workspaceID)
			if err != nil && !errors.Is(err, pgx.ErrNoRows) {
				return reasons, err
			}
			*githubConnected = err == nil && len(installations) > 0
		}
		if !*githubConnected {
			reasons = append(reasons, "Connect GitHub in this workspace.")
		}
	case "slack":
		var cfg automationTriggerReadinessConfig
		if json.Unmarshal(trigger.Config, &cfg) != nil {
			reasons = append(reasons, "Slack trigger configuration is invalid.")
			break
		}
		installationID := strings.TrimSpace(cfg.InstallationID)
		if installationID == "" {
			installations, err := s.Queries.ListChannelInstallationsByWorkspace(ctx, db.ListChannelInstallationsByWorkspaceParams{
				WorkspaceID: workspaceID,
				ChannelType: "slack",
			})
			if err != nil && !errors.Is(err, pgx.ErrNoRows) {
				return reasons, err
			}
			connected := false
			for _, installation := range installations {
				if installation.Status == "installed" && !installation.HostedPausedAt.Valid {
					connected = true
					break
				}
			}
			if !connected {
				reasons = append(reasons, "Connect a Slack workspace in this workspace.")
			}
		} else {
			parsed, err := util.ParseUUID(installationID)
			if err != nil {
				reasons = append(reasons, "Select a valid Slack workspace.")
			} else {
				installation, err := s.Queries.GetChannelInstallationInWorkspace(ctx, db.GetChannelInstallationInWorkspaceParams{
					ID: parsed, WorkspaceID: workspaceID, ChannelType: "slack",
				})
				if err != nil && !errors.Is(err, pgx.ErrNoRows) {
					return reasons, err
				}
				if errors.Is(err, pgx.ErrNoRows) || installation.Status != "installed" || installation.HostedPausedAt.Valid {
					reasons = append(reasons, "Connect the selected Slack workspace in this workspace.")
				}
			}
		}
		if preset == "slack.message" || preset == "slack.reaction" {
			channel := strings.TrimSpace(cfg.Channel)
			if channel == "" {
				reasons = append(reasons, "Select a Slack channel.")
			} else if !strings.HasPrefix(channel, "C") && !strings.HasPrefix(channel, "G") && !strings.HasPrefix(channel, "D") {
				reasons = append(reasons, "Select a valid Slack channel.")
			}
		}
	case "linear":
		if !*linearChecked {
			*linearChecked = true
			connection, err := s.Queries.GetLinearConnectionForWorkspace(ctx, workspaceID)
			if err != nil && !errors.Is(err, pgx.ErrNoRows) {
				return reasons, err
			}
			*linearConnected = err == nil && strings.EqualFold(connection.Status, "active")
		}
		if !*linearConnected {
			reasons = append(reasons, "Connect Linear in this workspace.")
		}
	default:
		reasons = append(reasons, "Trigger provider is not configured.")
	}
	return reasons, nil
}
