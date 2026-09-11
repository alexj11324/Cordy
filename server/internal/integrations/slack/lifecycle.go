package slack

import (
	"encoding/json"

	"github.com/slack-go/slack/slackevents"
)

// shouldRevokeSlackInstall reports whether this Events API delivery means the
// bot token must be retired: the workspace uninstalled the app, or Slack
// revoked the bot token. User-token-only revocations are ignored — they do
// not take the bot offline.
func shouldRevokeSlackInstall(event slackevents.EventsAPIEvent, body []byte) bool {
	switch lifecycleEventType(event, body) {
	case string(slackevents.AppUninstalled):
		return true
	case string(slackevents.TokensRevoked):
		if tre, ok := event.InnerEvent.Data.(*slackevents.TokensRevokedEvent); ok {
			return len(tre.Tokens.Bot) > 0
		}
		return botTokensRevoked(body)
	default:
		return false
	}
}

func lifecycleEventType(event slackevents.EventsAPIEvent, body []byte) string {
	if event.InnerEvent.Type != "" {
		return event.InnerEvent.Type
	}
	var envelope struct {
		Event struct {
			Type string `json:"type"`
		} `json:"event"`
	}
	if json.Unmarshal(body, &envelope) != nil {
		return ""
	}
	return envelope.Event.Type
}

func botTokensRevoked(body []byte) bool {
	var envelope struct {
		Event struct {
			Type   string `json:"type"`
			Tokens struct {
				Bot []string `json:"bot"`
			} `json:"tokens"`
		} `json:"event"`
	}
	if json.Unmarshal(body, &envelope) != nil {
		return false
	}
	return envelope.Event.Type == string(slackevents.TokensRevoked) && len(envelope.Event.Tokens.Bot) > 0
}
