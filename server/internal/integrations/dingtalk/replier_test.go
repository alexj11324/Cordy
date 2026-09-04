package dingtalk

import (
	"context"
	"encoding/json"
	"testing"

	"github.com/jackc/pgx/v5/pgtype"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel/engine"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func TestHubReplyPreservesGroupTarget(t *testing.T) {
	d := newDingtalkSendServer(t)
	r := NewOutboundReplier(OutboundReplierConfig{Client: NewClient(nil, d.srv.URL)})
	inst := engine.ResolvedInstallation{Platform: db.ChannelInstallation{
		Config: []byte(`{"app_id":"robot-1","robot_code":"robot-1","app_secret_encrypted":"Zml4dHVyZQ=="}`),
	}}
	msg := channel.InboundMessage{Source: channel.Source{ChatID: "group-fixture", ChatType: channel.ChatTypeGroup}}
	const reply = "已切换到 Reviewer"
	r.Reply(context.Background(), inst, msg, engine.Result{Outcome: engine.OutcomeHubCommand, ReplyText: reply})
	if d.lastPath != pathSendGroup || d.lastBody["openConversationId"] != "group-fixture" {
		t.Fatalf("Hub reply target = %s %+v", d.lastPath, d.lastBody)
	}
	var payload struct {
		Text string `json:"text"`
	}
	raw, _ := d.lastBody["msgParam"].(string)
	if err := json.Unmarshal([]byte(raw), &payload); err != nil || payload.Text != reply {
		t.Fatalf("Hub reply body = %q, error = %v", raw, err)
	}
}

func TestIssueCreatedText(t *testing.T) {
	issueID := pgtype.UUID{Valid: true}
	if got := issueCreatedText(engine.Result{IssueID: issueID, IssueIdentifier: "MUL-42", IssueTitle: "Fix login"}); got != "✅ Created MUL-42 — Fix login" {
		t.Fatalf("got %q", got)
	}
	if got := issueCreatedText(engine.Result{IssueID: issueID, IssueNumber: 7}); got != "✅ Created #7" {
		t.Fatalf("fallback got %q", got)
	}
}

func TestIssueDuplicateText(t *testing.T) {
	issueID := pgtype.UUID{Bytes: [16]byte{9}, Valid: true}
	got := issueDuplicateText(engine.Result{
		IssueID: issueID, IssueIdentifier: "MUL-42", IssueTitle: "Fix login", IssueDuplicate: true,
	})
	if got != "⚠️ Not created — active issue MUL-42 already exists: Fix login" {
		t.Fatalf("duplicate text = %q", got)
	}
}

func TestIssueUsageCopy(t *testing.T) {
	if issueUsageText != "Please include an issue title. Use:\n\n`/issue <title>`\n\n`[description]` (optional)" {
		t.Fatalf("plain issue usage copy = %q", issueUsageText)
	}
	if issueUsageWithMediaText != "Please add a title and resend with the image (*image can come before or after the command*):\n\n`/issue <title>`\n\n`[description]` (optional)" {
		t.Fatalf("media issue usage copy = %q", issueUsageWithMediaText)
	}
}

func TestDroppedReplyText(t *testing.T) {
	issueMsg := channel.InboundMessage{Text: "[Image]", CommandText: "/issue login is broken", AddressedToBot: true}
	cases := []struct {
		name string
		res  engine.Result
		msg  channel.InboundMessage
		want string
	}{
		{"non-member /issue gets refusal",
			engine.Result{Outcome: engine.OutcomeDropped, DropReason: engine.DropReasonNonWorkspaceMember},
			issueMsg, issueNotMemberText},
		{"revoked installation /issue gets disconnected notice",
			engine.Result{Outcome: engine.OutcomeDropped, DropReason: engine.DropReasonRevokedInstallation},
			issueMsg, issueDisabledText},
		{"duplicate /issue stays silent",
			engine.Result{Outcome: engine.OutcomeDropped, DropReason: engine.DropReasonDuplicate},
			issueMsg, ""},
		{"non-member plain chat stays silent",
			engine.Result{Outcome: engine.OutcomeDropped, DropReason: engine.DropReasonNonWorkspaceMember},
			channel.InboundMessage{Text: "hello", AddressedToBot: true}, ""},
		{"unaddressed group /issue stays silent",
			engine.Result{Outcome: engine.OutcomeDropped, DropReason: engine.DropReasonNonWorkspaceMember},
			channel.InboundMessage{Text: "/issue x", AddressedToBot: false}, ""},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := droppedReplyText(tc.res, tc.msg); got != tc.want {
				t.Fatalf("got %q, want %q", got, tc.want)
			}
		})
	}
}
