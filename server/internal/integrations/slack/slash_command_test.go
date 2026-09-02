package slack

import (
	"bytes"
	"context"
	"errors"
	"log/slog"
	"strings"
	"testing"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/slack-go/slack"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel/engine"
	"github.com/patchbay-ai/patchbay/server/internal/service"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// ---- fakes ----

type fakeSlashQueries struct {
	inst      db.ChannelInstallation
	instErr   error
	binding   db.ChannelUserBinding
	bindErr   error
	memberErr error
	gotAppID  string
	claims    map[string]bool
}

func (f *fakeSlashQueries) GetChannelInstallationByAppID(_ context.Context, arg db.GetChannelInstallationByAppIDParams) (db.ChannelInstallation, error) {
	f.gotAppID = arg.AppID
	return f.inst, f.instErr
}

func (f *fakeSlashQueries) GetChannelUserBindingByUserID(_ context.Context, _ db.GetChannelUserBindingByUserIDParams) (db.ChannelUserBinding, error) {
	return f.binding, f.bindErr
}

func (f *fakeSlashQueries) GetMemberByUserAndWorkspace(_ context.Context, _ db.GetMemberByUserAndWorkspaceParams) (db.Member, error) {
	return db.Member{}, f.memberErr
}

func (f *fakeSlashQueries) ClaimChannelInboundDedup(_ context.Context, arg db.ClaimChannelInboundDedupParams) (db.ChannelInboundMessageDedup, error) {
	if f.claims == nil {
		f.claims = map[string]bool{}
	}
	if f.claims[arg.MessageID] {
		return db.ChannelInboundMessageDedup{}, pgx.ErrNoRows
	}
	f.claims[arg.MessageID] = true
	return db.ChannelInboundMessageDedup{ClaimToken: slashTestUUID(9)}, nil
}

func (f *fakeSlashQueries) MarkChannelInboundDedupProcessed(_ context.Context, _ db.MarkChannelInboundDedupProcessedParams) (int64, error) {
	return 1, nil
}

func (f *fakeSlashQueries) ReleaseChannelInboundDedup(_ context.Context, arg db.ReleaseChannelInboundDedupParams) (int64, error) {
	delete(f.claims, arg.MessageID)
	return 1, nil
}

// fakeQuickCreate records the last EnqueueQuickCreateTask call so tests can
// assert the prompt is passed through verbatim and attributed correctly.
type fakeQuickCreate struct {
	task  db.AgentTaskQueue
	err   error
	calls int

	workspaceID pgtype.UUID
	requesterID pgtype.UUID
	agentID     pgtype.UUID
	teamID     pgtype.UUID
	prompt      string
}

type fakeSlashControlStarter struct {
	newCalls   int
	clearCalls int
	envelopeID string
	cmd        slack.SlashCommand
	err        error
}

func (f *fakeSlashControlStarter) StartSlackDMChat(_ context.Context, _ engine.ResolvedInstallation, _ pgtype.UUID, cmd slack.SlashCommand, envelopeID string) error {
	f.newCalls++
	f.cmd = cmd
	f.envelopeID = envelopeID
	return f.err
}

func (f *fakeSlashControlStarter) ClearSlackDMContext(_ context.Context, _ engine.ResolvedInstallation, _ pgtype.UUID, cmd slack.SlashCommand, envelopeID string) error {
	f.clearCalls++
	f.cmd = cmd
	f.envelopeID = envelopeID
	return f.err
}

func (f *fakeQuickCreate) EnqueueQuickCreateTask(_ context.Context, workspaceID, requesterID, agentID, teamID pgtype.UUID, prompt, _, _ string, _, _ pgtype.UUID, _ []pgtype.UUID) (db.AgentTaskQueue, error) {
	f.calls++
	f.workspaceID = workspaceID
	f.requesterID = requesterID
	f.agentID = agentID
	f.teamID = teamID
	f.prompt = prompt
	return f.task, f.err
}

func slashTestUUID(b byte) pgtype.UUID {
	var u pgtype.UUID
	for i := range u.Bytes {
		u.Bytes[i] = b
	}
	u.Valid = true
	return u
}

// newTestSlashProcessor builds a processor over fakes and returns it plus a
// pointer to the last ephemeral reply text and the reply count.
func newTestSlashProcessor(q slashQueries, tasks quickCreateEnqueuer, binding bindingMinter) (*SlashCommandProcessor, *string, *int) {
	captured := new(string)
	count := new(int)
	p := &SlashCommandProcessor{
		q:           q,
		tasks:       tasks,
		binding:     binding,
		appURL:      "https://app.example",
		bindingPath: "/slack/bind",
		logger:      slog.Default(),
	}
	p.respond = func(_ context.Context, _ string, text string) error {
		*count++
		*captured = text
		return nil
	}
	return p, captured, count
}

func activeSlashInstallation() db.ChannelInstallation {
	return db.ChannelInstallation{
		ID:              slashTestUUID(1),
		WorkspaceID:     slashTestUUID(2),
		AgentID:         slashTestUUID(3),
		InstallerUserID: slashTestUUID(4),
		Status:          "active",
		Config:          []byte(`{"app_id":"A1","team_id":"T1"}`),
	}
}

func issueSlashCmd() slack.SlashCommand {
	return slack.SlashCommand{
		Command:     "/issue",
		Text:        "Fix login",
		APIAppID:    "A1",
		TeamID:      "T1",
		UserID:      "U1",
		ChannelID:   "C1",
		ResponseURL: "https://hooks.slack.test/response",
	}
}

// ---- tests ----

func TestSlashHandle_EnqueuesQuickCreateAndAcks(t *testing.T) {
	q := &fakeSlashQueries{
		inst:    activeSlashInstallation(),
		binding: db.ChannelUserBinding{PatchbayUserID: slashTestUUID(9)},
	}
	tasks := &fakeQuickCreate{}
	p, captured, count := newTestSlashProcessor(q, tasks, &fakeBindingMinter{})

	p.Handle(context.Background(), issueSlashCmd())

	if tasks.calls != 1 {
		t.Fatalf("expected 1 quick-create enqueue, got %d", tasks.calls)
	}
	if *count != 1 {
		t.Fatalf("expected 1 ephemeral reply, got %d", *count)
	}
	if *captured != slashQueuedText {
		t.Fatalf("expected queued ack, got %q", *captured)
	}
	if q.gotAppID != "A1" {
		t.Errorf("installation lookup used app id %q, want A1", q.gotAppID)
	}
	if tasks.prompt != "Fix login" {
		t.Errorf("quick-create prompt = %q, want Fix login", tasks.prompt)
	}
	if tasks.workspaceID != slashTestUUID(2) {
		t.Errorf("quick-create workspace is not the installation workspace")
	}
	if tasks.agentID != slashTestUUID(3) {
		t.Errorf("quick-create not dispatched to the installation agent")
	}
	if tasks.requesterID != slashTestUUID(9) {
		t.Errorf("quick-create requester is not the bound member")
	}
	if tasks.teamID.Valid {
		t.Errorf("slash-command quick-create must not carry a team id")
	}
}

func TestSlashHandle_MultilinePromptPassedThrough(t *testing.T) {
	q := &fakeSlashQueries{
		inst:    activeSlashInstallation(),
		binding: db.ChannelUserBinding{PatchbayUserID: slashTestUUID(9)},
	}
	tasks := &fakeQuickCreate{}
	p, _, _ := newTestSlashProcessor(q, tasks, &fakeBindingMinter{})

	cmd := issueSlashCmd()
	cmd.Text = "  Title\nline one\nline two  "
	p.Handle(context.Background(), cmd)

	// The whole (trimmed) natural-language text is the prompt — no title/body
	// split; the agent authors the well-formed issue from it.
	if tasks.prompt != "Title\nline one\nline two" {
		t.Errorf("prompt = %q, want the full trimmed text", tasks.prompt)
	}
}

func TestSlashHandle_EmptyPromptIsUsage(t *testing.T) {
	tasks := &fakeQuickCreate{}
	p, captured, count := newTestSlashProcessor(&fakeSlashQueries{inst: activeSlashInstallation()}, tasks, &fakeBindingMinter{})

	cmd := issueSlashCmd()
	cmd.Text = "   "
	p.Handle(context.Background(), cmd)

	if tasks.calls != 0 {
		t.Fatalf("empty prompt must not enqueue a task")
	}
	if *count != 1 || *captured != slashUsageText {
		t.Fatalf("expected usage reply, got %q", *captured)
	}
}

func TestSlashHandle_UnboundUserGetsLink(t *testing.T) {
	q := &fakeSlashQueries{inst: activeSlashInstallation(), bindErr: pgx.ErrNoRows}
	tasks := &fakeQuickCreate{}
	bind := &fakeBindingMinter{raw: "TOKEN123"}
	p, captured, _ := newTestSlashProcessor(q, tasks, bind)

	p.Handle(context.Background(), issueSlashCmd())

	if tasks.calls != 0 {
		t.Fatalf("unbound user must not enqueue a task")
	}
	if bind.calls != 1 {
		t.Fatalf("expected a binding token to be minted, got %d", bind.calls)
	}
	if !strings.Contains(*captured, "link your account") || !strings.Contains(*captured, "TOKEN123") {
		t.Fatalf("reply missing bind link: %q", *captured)
	}
}

func TestSlashHandle_NonMemberDropped(t *testing.T) {
	q := &fakeSlashQueries{
		inst:      activeSlashInstallation(),
		binding:   db.ChannelUserBinding{PatchbayUserID: slashTestUUID(9)},
		memberErr: pgx.ErrNoRows,
	}
	tasks := &fakeQuickCreate{}
	p, captured, _ := newTestSlashProcessor(q, tasks, &fakeBindingMinter{})

	p.Handle(context.Background(), issueSlashCmd())

	if tasks.calls != 0 {
		t.Fatalf("non-member must not enqueue a task")
	}
	if *captured != slashNotMemberText {
		t.Fatalf("expected not-member reply, got %q", *captured)
	}
}

func TestSlashHandle_InactiveInstallation(t *testing.T) {
	inst := activeSlashInstallation()
	inst.Status = "revoked"
	tasks := &fakeQuickCreate{}
	p, captured, _ := newTestSlashProcessor(&fakeSlashQueries{inst: inst}, tasks, &fakeBindingMinter{})

	p.Handle(context.Background(), issueSlashCmd())

	if tasks.calls != 0 || *captured != slashDisabledText {
		t.Fatalf("inactive install: calls=%d reply=%q", tasks.calls, *captured)
	}
}

func TestSlashHandle_TeamMismatchTreatedAsDisconnected(t *testing.T) {
	tasks := &fakeQuickCreate{}
	p, captured, _ := newTestSlashProcessor(&fakeSlashQueries{inst: activeSlashInstallation()}, tasks, &fakeBindingMinter{})

	cmd := issueSlashCmd()
	cmd.TeamID = "T2" // config team is T1
	p.Handle(context.Background(), cmd)

	if tasks.calls != 0 || *captured != slashDisabledText {
		t.Fatalf("team mismatch: calls=%d reply=%q", tasks.calls, *captured)
	}
}

func TestSlashHandle_EnqueueFailureIsInternalError(t *testing.T) {
	q := &fakeSlashQueries{
		inst:    activeSlashInstallation(),
		binding: db.ChannelUserBinding{PatchbayUserID: slashTestUUID(9)},
	}
	tasks := &fakeQuickCreate{err: errors.New("agent has no runtime")}
	p, captured, _ := newTestSlashProcessor(q, tasks, &fakeBindingMinter{})

	p.Handle(context.Background(), issueSlashCmd())

	if tasks.calls != 1 {
		t.Fatalf("expected the enqueue to be attempted once, got %d", tasks.calls)
	}
	if *captured != slashInternalErrorText {
		t.Fatalf("expected internal-error reply, got %q", *captured)
	}
}

func TestSlashHandle_IssueLimitReachedIsActionable(t *testing.T) {
	q := &fakeSlashQueries{
		inst:    activeSlashInstallation(),
		binding: db.ChannelUserBinding{PatchbayUserID: slashTestUUID(9)},
	}
	tasks := &fakeQuickCreate{err: &service.IssueLimitReachedError{Limit: 100, PolicyRevision: 7}}
	p, captured, _ := newTestSlashProcessor(q, tasks, &fakeBindingMinter{})
	var logs bytes.Buffer
	p.logger = slog.New(slog.NewTextHandler(&logs, nil))

	p.Handle(context.Background(), issueSlashCmd())

	if tasks.calls != 1 {
		t.Fatalf("expected the enqueue to be attempted once, got %d", tasks.calls)
	}
	if *captured != slashIssueLimitText {
		t.Fatalf("expected issue-limit reply, got %q", *captured)
	}
	if logs.Len() != 0 {
		t.Fatalf("expected issue-limit rejection not to emit a warning, got %q", logs.String())
	}
}

func TestSlashHandle_IgnoresOtherCommands(t *testing.T) {
	tasks := &fakeQuickCreate{}
	p, _, count := newTestSlashProcessor(&fakeSlashQueries{inst: activeSlashInstallation()}, tasks, &fakeBindingMinter{})

	cmd := issueSlashCmd()
	cmd.Command = "/other"
	p.Handle(context.Background(), cmd)

	if tasks.calls != 0 || *count != 0 {
		t.Fatalf("non-/issue command must be ignored: calls=%d replies=%d", tasks.calls, *count)
	}
}

func TestSlashHandle_NewDMUsesSharedStarterAndEnvelopeDedup(t *testing.T) {
	q := &fakeSlashQueries{inst: activeSlashInstallation(), binding: db.ChannelUserBinding{PatchbayUserID: slashTestUUID(9)}}
	tasks := &fakeQuickCreate{}
	p, captured, _ := newTestSlashProcessor(q, tasks, &fakeBindingMinter{})
	starter := &fakeSlashControlStarter{}
	p.control = starter
	cmd := issueSlashCmd()
	cmd.Command, cmd.ChannelID, cmd.Text = "/new", "D1", "new topic"
	p.HandleEnvelope(context.Background(), cmd, "env-123")
	if starter.newCalls != 1 || starter.clearCalls != 0 || starter.envelopeID != "env-123" || starter.cmd.Text != "new topic" {
		t.Fatalf("starter=%+v", starter)
	}
	if tasks.calls != 0 {
		t.Fatalf("/new must not also enqueue /issue quick-create work, got %d calls", tasks.calls)
	}
	if *captured != slashNewStartedText {
		t.Fatalf("reply=%q", *captured)
	}
}

func TestSlashHandle_NewChannelGuidesToMentionWithoutGuessingThread(t *testing.T) {
	q := &fakeSlashQueries{
		inst:    activeSlashInstallation(),
		binding: db.ChannelUserBinding{PatchbayUserID: slashTestUUID(9)},
	}
	p, captured, _ := newTestSlashProcessor(q, &fakeQuickCreate{}, &fakeBindingMinter{})
	starter := &fakeSlashControlStarter{}
	p.control = starter
	cmd := issueSlashCmd()
	cmd.Command, cmd.ChannelID = "/new", "C1"
	p.HandleEnvelope(context.Background(), cmd, "env-123")
	if starter.newCalls != 0 || starter.clearCalls != 0 {
		t.Fatal("channel slash command must not rotate a guessed route")
	}
	if *captured != slashNewThreadGuideText {
		t.Fatalf("reply=%q", *captured)
	}
}

func TestSlashHandle_ClearDMUsesSharedStarterAndEnvelopeDedup(t *testing.T) {
	q := &fakeSlashQueries{inst: activeSlashInstallation(), binding: db.ChannelUserBinding{PatchbayUserID: slashTestUUID(9)}}
	p, captured, _ := newTestSlashProcessor(q, &fakeQuickCreate{}, &fakeBindingMinter{})
	starter := &fakeSlashControlStarter{}
	p.control = starter
	cmd := issueSlashCmd()
	cmd.Command, cmd.ChannelID, cmd.Text = "/clear", "D1", "start clean"
	p.HandleEnvelope(context.Background(), cmd, "env-clear-123")
	if starter.clearCalls != 1 || starter.newCalls != 0 || starter.envelopeID != "env-clear-123" || starter.cmd.Text != "start clean" {
		t.Fatalf("starter=%+v", starter)
	}
	if *captured != slashClearStartedText {
		t.Fatalf("reply=%q", *captured)
	}
}

func TestSlashHandle_ClearChannelGuidesToMentionWithoutGuessingThread(t *testing.T) {
	q := &fakeSlashQueries{inst: activeSlashInstallation(), binding: db.ChannelUserBinding{PatchbayUserID: slashTestUUID(9)}}
	p, captured, _ := newTestSlashProcessor(q, &fakeQuickCreate{}, &fakeBindingMinter{})
	starter := &fakeSlashControlStarter{}
	p.control = starter
	cmd := issueSlashCmd()
	cmd.Command, cmd.ChannelID = "/clear", "C1"
	p.HandleEnvelope(context.Background(), cmd, "env-clear-123")
	if starter.newCalls != 0 || starter.clearCalls != 0 {
		t.Fatal("channel slash command must not clear a guessed route")
	}
	if *captured != slashClearThreadGuideText {
		t.Fatalf("reply=%q", *captured)
	}
}

func TestSlashHandle_ReplayCollapsesOntoOneIssue(t *testing.T) {
	q := &fakeSlashQueries{
		inst:    activeSlashInstallation(),
		binding: db.ChannelUserBinding{PatchbayUserID: slashTestUUID(9)},
	}
	tasks := &fakeQuickCreate{}
	p, captured, count := newTestSlashProcessor(q, tasks, &fakeBindingMinter{})

	cmd := issueSlashCmd()
	cmd.TriggerID = "13345224609.738474920.8088930"
	p.Handle(context.Background(), cmd)
	// The replay carries the identical trigger_id: same ack, no second issue.
	p.Handle(context.Background(), cmd)

	if tasks.calls != 1 {
		t.Fatalf("replay must not file a second issue, got %d enqueues", tasks.calls)
	}
	if *count != 2 || *captured != slashQueuedText {
		t.Fatalf("replay must repeat the ack (replies=%d, last=%q)", *count, *captured)
	}
}

func TestSlashHandle_FreshTriggerFilesAgain(t *testing.T) {
	q := &fakeSlashQueries{
		inst:    activeSlashInstallation(),
		binding: db.ChannelUserBinding{PatchbayUserID: slashTestUUID(9)},
	}
	tasks := &fakeQuickCreate{}
	p, _, _ := newTestSlashProcessor(q, tasks, &fakeBindingMinter{})

	first := issueSlashCmd()
	first.TriggerID = "13345224609.738474920.8088930"
	second := issueSlashCmd()
	second.TriggerID = "13345224609.738474920.9999999"
	p.Handle(context.Background(), first)
	p.Handle(context.Background(), second)

	if tasks.calls != 2 {
		t.Fatalf("distinct invocations must file distinct issues, got %d", tasks.calls)
	}
}

func TestSlashHandle_FailedEnqueueReleasesForRetry(t *testing.T) {
	q := &fakeSlashQueries{
		inst:    activeSlashInstallation(),
		binding: db.ChannelUserBinding{PatchbayUserID: slashTestUUID(9)},
	}
	tasks := &fakeQuickCreate{err: errors.New("queue down")}
	p, captured, _ := newTestSlashProcessor(q, tasks, &fakeBindingMinter{})

	cmd := issueSlashCmd()
	cmd.TriggerID = "13345224609.738474920.8088930"
	p.Handle(context.Background(), cmd)
	if *captured != slashInternalErrorText {
		t.Fatalf("failed enqueue reply = %q, want internal error", *captured)
	}
	// The failure releases the claim, so the invoker's retry files the issue.
	tasks.err = nil
	p.Handle(context.Background(), cmd)
	if tasks.calls != 2 {
		t.Fatalf("retry after failure must re-enqueue, got %d calls", tasks.calls)
	}
	if *captured != slashQueuedText {
		t.Fatalf("retry reply = %q, want queued ack", *captured)
	}
}

func TestSlashDedupKey(t *testing.T) {
	if got := slashDedupKey("13345224609.738474920.8088930"); got != "slash:13345224609.738474920.8088930" {
		t.Fatalf("key = %q", got)
	}
	if got := slashDedupKey(""); got != "" {
		t.Fatalf("empty trigger must have no key, got %q", got)
	}
	if got := slashDedupKey("   "); got != "" {
		t.Fatalf("blank trigger must have no key, got %q", got)
	}
}
