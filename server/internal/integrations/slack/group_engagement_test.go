package slack

import (
	"context"
	"errors"
	"testing"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel/engine"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

type fakeEngagementQueries struct {
	row db.ChannelChatSessionBinding
	err error
}

func (f *fakeEngagementQueries) GetChannelChatSessionBinding(_ context.Context, _ db.GetChannelChatSessionBindingParams) (db.ChannelChatSessionBinding, error) {
	return f.row, f.err
}

func groupThreadMessage() channel.InboundMessage {
	return channel.InboundMessage{
		EventID:   "evt-2",
		MessageID: "1700000000.000200",
		Type:      channel.MsgTypeText,
		Text:      "and then?",
		Source: channel.Source{
			ChannelType: TypeSlack,
			ChatID:      "C123",
			ChatType:    channel.ChatTypeGroup,
			SenderID:    "U456",
			ThreadID:    "1700000000.000100",
		},
	}
}

func TestEngagedInThread_FollowsUpWithoutMention(t *testing.T) {
	c := &engagementChecker{q: &fakeEngagementQueries{}}
	inst := engine.ResolvedInstallation{ID: pgtype.UUID{Bytes: [16]byte{1}, Valid: true}}
	engaged, err := c.EngagedInThread(context.Background(), inst, groupThreadMessage())
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !engaged {
		t.Fatal("live binding for the thread root must count as engaged")
	}
}

func TestEngagedInThread_NoBindingMeansNewThread(t *testing.T) {
	c := &engagementChecker{q: &fakeEngagementQueries{err: pgx.ErrNoRows}}
	inst := engine.ResolvedInstallation{ID: pgtype.UUID{Bytes: [16]byte{1}, Valid: true}}
	engaged, err := c.EngagedInThread(context.Background(), inst, groupThreadMessage())
	if err != nil {
		t.Fatalf("missing row must not be an error: %v", err)
	}
	if engaged {
		t.Fatal("thread with no binding must not count as engaged")
	}
}

func TestEngagedInThread_TopLevelStartsNewRoot(t *testing.T) {
	calls := 0
	q := &countEngagementQueries{inner: &fakeEngagementQueries{}, calls: &calls}
	c := &engagementChecker{q: q}
	inst := engine.ResolvedInstallation{ID: pgtype.UUID{Bytes: [16]byte{1}, Valid: true}}
	msg := groupThreadMessage()
	msg.Source.ThreadID = ""
	top, err := c.EngagedInThread(context.Background(), inst, msg)
	if err != nil || top {
		t.Fatalf("top-level message must never engage (engaged=%v, err=%v)", top, err)
	}
	msg.Source.ThreadID = msg.MessageID
	root, err := c.EngagedInThread(context.Background(), inst, msg)
	if err != nil || root {
		t.Fatalf("thread root must never engage (engaged=%v, err=%v)", root, err)
	}
	if calls != 0 {
		t.Fatalf("non-continuations must short-circuit without a store read, got %d", calls)
	}
}

func TestEngagedInThread_DMShortCircuits(t *testing.T) {
	calls := 0
	q := &countEngagementQueries{inner: &fakeEngagementQueries{}, calls: &calls}
	c := &engagementChecker{q: q}
	inst := engine.ResolvedInstallation{ID: pgtype.UUID{Bytes: [16]byte{1}, Valid: true}}
	msg := groupThreadMessage()
	msg.Source.ChatType = channel.ChatTypeP2P
	engaged, err := c.EngagedInThread(context.Background(), inst, msg)
	if err != nil || engaged {
		t.Fatalf("DM must never consult engagement (engaged=%v, err=%v)", engaged, err)
	}
	if calls != 0 {
		t.Fatalf("DM must short-circuit without a store read, got %d", calls)
	}
}

func TestEngagedInThread_StoreErrorSurfaces(t *testing.T) {
	c := &engagementChecker{q: &fakeEngagementQueries{err: errors.New("db down")}}
	inst := engine.ResolvedInstallation{ID: pgtype.UUID{Bytes: [16]byte{1}, Valid: true}}
	if _, err := c.EngagedInThread(context.Background(), inst, groupThreadMessage()); err == nil {
		t.Fatal("store error must surface so the router can release the claim")
	}
}

type countEngagementQueries struct {
	inner engagementQueries
	calls *int
}

func (q *countEngagementQueries) GetChannelChatSessionBinding(ctx context.Context, arg db.GetChannelChatSessionBindingParams) (db.ChannelChatSessionBinding, error) {
	*q.calls++
	return q.inner.GetChannelChatSessionBinding(ctx, arg)
}
