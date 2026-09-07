package slack

import (
	"context"
	"fmt"
	"strings"

	"github.com/jackc/pgx/v5/pgtype"
	slackapi "github.com/slack-go/slack"

	"github.com/orvilo-ai/orvilo/server/internal/util"
)

type AutomationCatalogChannel struct {
	InstallationID string `json:"installation_id"`
	TeamID         string `json:"team_id"`
	ID             string `json:"id"`
	Name           string `json:"name"`
}

type AutomationCatalog struct {
	Channels []AutomationCatalogChannel `json:"channels"`
}

// AutomationCatalog lists the public channels visible to each installed Slack
// bot. Every item retains its installation identity because Slack channel IDs
// are only meaningful inside that provider workspace.
func (s *InstallService) AutomationCatalog(ctx context.Context, workspaceID pgtype.UUID) (AutomationCatalog, error) {
	catalog := AutomationCatalog{Channels: []AutomationCatalogChannel{}}
	installations, err := s.ListByWorkspace(ctx, workspaceID)
	if err != nil {
		return catalog, fmt.Errorf("list Slack installations: %w", err)
	}
	for _, inst := range installations {
		if inst.Status != "installed" || inst.HostedPausedAt.Valid {
			continue
		}
		creds, err := decodeCredentials(inst.Config, s.box.Open)
		if err != nil {
			return catalog, fmt.Errorf("decode Slack installation %s: %w", util.UUIDToString(inst.ID), err)
		}
		client := slackapi.New(creds.BotToken, s.slackOpts()...)
		installationID := util.UUIDToString(inst.ID)
		cursor := ""
		for page := 0; page < 50; page++ {
			channels, next, err := client.GetConversationsContext(ctx, &slackapi.GetConversationsParameters{
				Cursor: cursor, ExcludeArchived: true, Limit: 200,
				Types: []string{"public_channel"}, TeamID: creds.TeamID,
			})
			if err != nil {
				return catalog, fmt.Errorf("list Slack channels for installation %s: %w", installationID, err)
			}
			for _, channel := range channels {
				if channel.ID == "" || channel.Name == "" || channel.IsArchived || !channel.IsMember {
					continue
				}
				catalog.Channels = append(catalog.Channels, AutomationCatalogChannel{
					InstallationID: installationID, TeamID: creds.TeamID, ID: channel.ID, Name: channel.Name,
				})
			}
			cursor = strings.TrimSpace(next)
			if cursor == "" {
				break
			}
			if page == 49 {
				return catalog, fmt.Errorf("Slack channel catalog exceeded pagination limit for installation %s", installationID)
			}
		}
	}
	return catalog, nil
}
