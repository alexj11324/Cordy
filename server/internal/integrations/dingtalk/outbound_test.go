package dingtalk

import (
	"context"
	"errors"
	"testing"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/events"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

type noDeliveryOutboundQueries struct{}

func (noDeliveryOutboundQueries) GetChannelTaskDelivery(context.Context, pgtype.UUID) (db.ChannelTaskDelivery, error) {
	return db.ChannelTaskDelivery{}, pgx.ErrNoRows
}
func (noDeliveryOutboundQueries) GetAgentTask(context.Context, pgtype.UUID) (db.AgentTaskQueue, error) {
	panic("GetAgentTask must not run without a task delivery snapshot")
}
func (noDeliveryOutboundQueries) TaskHasChannelIngestedMessages(context.Context, pgtype.UUID) (bool, error) {
	panic("TaskHasChannelIngestedMessages must not run without a task delivery snapshot")
}
func (noDeliveryOutboundQueries) GetChannelInstallation(context.Context, db.GetChannelInstallationParams) (db.ChannelInstallation, error) {
	panic("GetChannelInstallation must not run without a task delivery snapshot")
}

func (noDeliveryOutboundQueries) GetChannelChatSessionBindingBySessionAny(context.Context, pgtype.UUID) (db.ChannelChatSessionBinding, error) {
	panic("binding lookup must not run without a task delivery snapshot")
}

type routedOutboundQueries struct {
	noDeliveryOutboundQueries
	binding db.ChannelChatSessionBinding
	err     error
}

func (routedOutboundQueries) GetChannelTaskDelivery(context.Context, pgtype.UUID) (db.ChannelTaskDelivery, error) {
	return db.ChannelTaskDelivery{BindingID: pgtype.UUID{Bytes: [16]byte{1}, Valid: true}, ChannelType: string(TypeDingTalk)}, nil
}

func (q routedOutboundQueries) GetChannelChatSessionBindingBySessionAny(context.Context, pgtype.UUID) (db.ChannelChatSessionBinding, error) {
	return q.binding, q.err
}

var errOutboundReachedTask = errors.New("valid snapshot reached task origin check")

func (routedOutboundQueries) GetAgentTask(context.Context, pgtype.UUID) (db.AgentTaskQueue, error) {
	return db.AgentTaskQueue{}, errOutboundReachedTask
}

func TestOutboundReassignmentRevokesOldDeliverySnapshot(t *testing.T) {
	for _, tc := range []struct {
		name    string
		binding db.ChannelChatSessionBinding
		err     error
		want    error
	}{
		{name: "binding removed by reassignment", err: pgx.ErrNoRows},
		{name: "another binding must not retarget snapshot", binding: db.ChannelChatSessionBinding{ID: pgtype.UUID{Bytes: [16]byte{2}, Valid: true}}},
		{name: "original generation remains valid", binding: db.ChannelChatSessionBinding{ID: pgtype.UUID{Bytes: [16]byte{1}, Valid: true}}, want: errOutboundReachedTask},
	} {
		t.Run(tc.name, func(t *testing.T) {
			o := NewOutbound(routedOutboundQueries{binding: tc.binding, err: tc.err}, nil, nil, nil)
			event := events.Event{
				Type: protocol.EventChatDone, TaskID: "11111111-1111-1111-1111-111111111111",
				ChatSessionID: "22222222-2222-2222-2222-222222222222",
				Payload: protocol.ChatDonePayload{Content: "reply from the original agent"},
			}
			if err := o.processEvent(context.Background(), event); !errors.Is(err, tc.want) {
				t.Fatalf("processEvent = %v, want %v", err, tc.want)
			}
		})
	}
}

func TestOutboundFailsClosedWithoutTaskDeliverySnapshot(t *testing.T) {
	o := NewOutbound(noDeliveryOutboundQueries{}, nil, nil, nil)
	event := events.Event{
		Type:          protocol.EventChatDone,
		TaskID:        "11111111-1111-1111-1111-111111111111",
		ChatSessionID: "22222222-2222-2222-2222-222222222222",
		Payload:       protocol.ChatDonePayload{Content: "must stay in Patchbay"},
	}
	if err := o.processEvent(context.Background(), event); err != nil {
		t.Fatalf("processEvent: %v", err)
	}
}

func TestEventContent(t *testing.T) {
	cases := []struct {
		name  string
		event events.Event
		want  string
	}{
		{"chat done typed", events.Event{Type: protocol.EventChatDone, Payload: protocol.ChatDonePayload{Content: "reply"}}, "reply"},
		{"map round trip", events.Event{Type: protocol.EventChatDone, Payload: map[string]any{"content": "from map"}}, "from map"},
		{"empty map", events.Event{Type: protocol.EventChatDone, Payload: map[string]any{}}, ""},
		{"nil", events.Event{Type: protocol.EventChatDone}, ""},
		{
			"task failed with error",
			events.Event{Type: protocol.EventTaskFailed, Payload: map[string]any{"error": "task timed out", "retry_pending": false}},
			"⚠️ task timed out",
		},
		{
			// Retry-pending failures stay silent even if a mixed-version
			// publisher accidentally includes an error string.
			"task failed with retry pending",
			events.Event{Type: protocol.EventTaskFailed, Payload: map[string]any{"error": "task timed out", "failure_reason": "timeout", "retry_pending": true}},
			"",
		},
		{
			// Failure broadcasts without an error text have nothing safe to
			// deliver and stay silent.
			"task failed without error",
			events.Event{Type: protocol.EventTaskFailed, Payload: map[string]any{"failure_reason": "timeout", "retry_pending": false}},
			"",
		},
		{
			// task:failed payloads never carry "content"; it must not leak
			// through the chat-done branch.
			"task failed ignores content key",
			events.Event{Type: protocol.EventTaskFailed, Payload: map[string]any{"content": "not for delivery"}},
			"",
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := eventContent(tc.event); got != tc.want {
				t.Fatalf("got %q, want %q", got, tc.want)
			}
		})
	}
}
