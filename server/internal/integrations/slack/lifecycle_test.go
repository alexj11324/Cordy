package slack

import (
	"testing"

	"github.com/slack-go/slack/slackevents"
)

func TestShouldRevokeSlackInstall(t *testing.T) {
	t.Parallel()

	botRevoked := &slackevents.TokensRevokedEvent{Type: "tokens_revoked"}
	botRevoked.Tokens.Bot = []string{"xoxb-1"}

	cases := []struct {
		name  string
		event slackevents.EventsAPIEvent
		body  []byte
		want  bool
	}{
		{
			name:  "app_uninstalled inner type",
			event: slackevents.EventsAPIEvent{InnerEvent: slackevents.EventsAPIInnerEvent{Type: string(slackevents.AppUninstalled)}},
			body:  []byte(`{"type":"event_callback","event":{"type":"app_uninstalled"}}`),
			want:  true,
		},
		{
			name: "bot tokens_revoked typed payload",
			event: slackevents.EventsAPIEvent{InnerEvent: slackevents.EventsAPIInnerEvent{
				Type: string(slackevents.TokensRevoked),
				Data: botRevoked,
			}},
			body: []byte(`{"type":"event_callback","event":{"type":"tokens_revoked","tokens":{"bot":["xoxb-1"]}}}`),
			want: true,
		},
		{
			name:  "bot tokens_revoked from body only",
			event: slackevents.EventsAPIEvent{},
			body:  []byte(`{"type":"event_callback","event":{"type":"tokens_revoked","tokens":{"bot":["xoxb-1"],"oauth":[]}}}`),
			want:  true,
		},
		{
			name:  "user-only tokens_revoked",
			event: slackevents.EventsAPIEvent{},
			body:  []byte(`{"type":"event_callback","event":{"type":"tokens_revoked","tokens":{"bot":[],"oauth":["xoxp-1"]}}}`),
			want:  false,
		},
		{
			name:  "ordinary mention",
			event: slackevents.EventsAPIEvent{InnerEvent: slackevents.EventsAPIInnerEvent{Type: string(slackevents.AppMention)}},
			body:  []byte(`{"type":"event_callback","event":{"type":"app_mention"}}`),
			want:  false,
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := shouldRevokeSlackInstall(tc.event, tc.body); got != tc.want {
				t.Fatalf("shouldRevokeSlackInstall = %v, want %v", got, tc.want)
			}
		})
	}
}
