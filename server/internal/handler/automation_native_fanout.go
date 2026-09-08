package handler

import (
	"context"
	"errors"
	"log/slog"
	"net/http"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"

	"github.com/orvilo-ai/orvilo/server/internal/service"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

// FanoutNativeAutomationEvent persists a queued webhook_delivery for every
// enabled native trigger in the workspace whose catalog preset matches, then
// wakes the existing delivery worker. Platform ingress already verified the
// request; failures here are logged and never fail the provider webhook.
func (h *Handler) FanoutNativeAutomationEvent(
	ctx context.Context,
	workspaceID pgtype.UUID,
	provider, preset, dedupeKey, dedupeSource string,
	body []byte,
	match service.NativeTriggerMatch,
) {
	if preset == "" {
		return
	}
	if len(body) > maxWebhookBodyBytes {
		// Native provider ingress is allowed to accept larger payloads for its
		// own consumers, but automation deliveries share the same bounded agent
		// context/storage contract as generic webhooks. Drop an oversized event
		// before it is copied once per matching trigger.
		slog.Warn("automation native fan-out: payload too large",
			"provider", provider,
			"preset", preset,
			"bytes", len(body),
			"max_bytes", maxWebhookBodyBytes,
		)
		return
	}
	triggers, err := h.Queries.ListEnabledAutomationTriggersForEvent(ctx, db.ListEnabledAutomationTriggersForEventParams{
		WorkspaceID: workspaceID,
		Provider:    provider,
		Preset:      pgtype.Text{String: preset, Valid: true},
	})
	if err != nil {
		slog.Warn("automation native fan-out: list triggers",
			"provider", provider,
			"preset", preset,
			"err", err,
		)
		return
	}
	headers := http.Header{}
	headers.Set("X-Event-Type", preset)
	headers.Set("Content-Type", "application/json")
	selected := selectedHeadersJSON(headers)
	notified := false
	for _, trigger := range triggers {
		if !service.TriggerConfigMatches(trigger.Config, match) {
			continue
		}
		_, dup, persistErr := h.persistInboundDeliveryCtx(ctx, persistDeliveryInput{
			WorkspaceID:     workspaceID,
			AutomationID:    trigger.AutomationID,
			TriggerID:       trigger.ID,
			Provider:        provider,
			Event:           preset,
			DedupeKey:       dedupeKey,
			DedupeSource:    dedupeSource,
			SignatureStatus: sigStatusValid,
			ContentType:     "application/json",
			RawBody:         body,
			SelectedHeaders: selected,
		})
		if persistErr != nil {
			slog.Warn("automation native fan-out: persist delivery",
				"trigger_id", uuidToString(trigger.ID),
				"preset", preset,
				"err", persistErr,
			)
			continue
		}
		if dup {
			continue
		}
		notified = true
	}
	if notified && h.WebhookDeliveryWorker != nil {
		h.WebhookDeliveryWorker.Notify()
	}
}

func (h *Handler) fanoutGitHubAutomations(ctx context.Context, event string, body []byte, deliveryID string) {
	preset := service.MapGitHubEventToPreset(event, "", body)
	if preset == "" {
		return
	}
	installationID := service.GitHubInstallationID(body)
	if installationID == 0 {
		return
	}
	insts, err := h.Queries.ListGitHubInstallationsByInstallationID(ctx, installationID)
	if err != nil {
		slog.Warn("automation native fan-out: list github installations",
			"installation_id", installationID,
			"err", err,
		)
		return
	}
	match := service.GitHubTriggerMatch(body)
	for _, inst := range insts {
		h.FanoutNativeAutomationEvent(ctx, inst.WorkspaceID, "github", preset, deliveryID, "github_delivery", body, match)
	}
}

func (h *Handler) HandleSlackNativeAutomation(ctx context.Context, inst db.ChannelInstallation, body []byte) {
	preset, eventID, match := service.ParseSlackNativeEnvelopeForInstallation(body, uuidToString(inst.ID))
	if preset == "" {
		return
	}
	if match.SenderID != "" {
		binding, err := h.Queries.GetChannelUserBindingByUserID(ctx, db.GetChannelUserBindingByUserIDParams{
			InstallationID: inst.ID, ChannelUserID: match.SenderID,
		})
		switch {
		case err == nil:
			_, memberErr := h.Queries.GetMemberByUserAndWorkspace(ctx, db.GetMemberByUserAndWorkspaceParams{
				UserID:      binding.OrviloUserID,
				WorkspaceID: inst.WorkspaceID,
			})
			switch {
			case memberErr == nil:
				match.SenderAuthenticated = true
			case errors.Is(memberErr, pgx.ErrNoRows):
				// A stale binding does not prove current workspace membership.
			default:
				slog.Warn("automation native fan-out: verify Slack sender membership",
					"installation_id", uuidToString(inst.ID), "sender_id", match.SenderID, "err", memberErr)
			}
		case errors.Is(err, pgx.ErrNoRows):
			// Anyone remains eligible; authenticated-only triggers reject below.
		default:
			slog.Warn("automation native fan-out: resolve Slack sender identity",
				"installation_id", uuidToString(inst.ID), "sender_id", match.SenderID, "err", err)
		}
	}
	h.FanoutNativeAutomationEvent(ctx, inst.WorkspaceID, "slack", preset, eventID, "slack_event", body, match)
}

func (h *Handler) fanoutLinearAutomations(ctx context.Context, connectionID pgtype.UUID, eventType, action, deliveryID string, body []byte) {
	preset := service.MapLinearEventToPreset(eventType, action, body)
	if preset == "" {
		return
	}
	conn, err := h.Queries.GetLinearConnectionByIDUnscoped(ctx, connectionID)
	if err != nil {
		slog.Warn("automation native fan-out: load linear connection",
			"connection_id", uuidToString(connectionID),
			"err", err,
		)
		return
	}
	h.FanoutNativeAutomationEvent(ctx, conn.WorkspaceID, "linear", preset, deliveryID, "linear_delivery", body, service.LinearTriggerMatch(body))
}
