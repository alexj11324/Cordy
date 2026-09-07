package handler

import (
	"context"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/orvilo-ai/orvilo/server/internal/events"
	"github.com/orvilo-ai/orvilo/server/internal/integrations/linearsync"
	"github.com/orvilo-ai/orvilo/server/internal/service"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

// LinearSyncWorker is the HTTP layer's complete view of Linear background
// synchronization. Keeping this boundary narrow prevents handlers from
// reaching into worker implementation details such as its provider client or
// lease state.
type LinearSyncWorker interface {
	Run(context.Context)
	Wake()
	AccessToken(context.Context, pgtype.UUID) (string, error)
	ResolveConflict(context.Context, pgtype.UUID, pgtype.UUID, pgtype.UUID, string, any) (db.LinearSyncConflict, db.Issue, error)
}

type linearEventSink struct {
	bus *events.Bus
}

var _ linearsync.EventSink = (*linearEventSink)(nil)

// NewLinearEventSink adapts synchronization events to the established
// workspace event envelope and payload shapes consumed by realtime clients.
func NewLinearEventSink(bus *events.Bus) linearsync.EventSink {
	return &linearEventSink{bus: bus}
}

func (s *linearEventSink) IssueChanged(issue db.Issue, eventType, actorType, actorID string) {
	if s == nil || s.bus == nil {
		return
	}
	s.bus.Publish(events.Event{
		Type:        eventType,
		WorkspaceID: uuidToString(issue.WorkspaceID),
		ActorType:   actorType,
		ActorID:     actorID,
		Payload:     map[string]any{"issue": service.IssueToMap(issue, "")},
		TaskID:      uuidToString(issue.ID),
	})
}

func (s *linearEventSink) CommentChanged(comment db.Comment, issueRevision int64, created bool) {
	if s == nil || s.bus == nil {
		return
	}
	eventType := protocol.EventCommentUpdated
	if created {
		eventType = protocol.EventCommentCreated
	}
	response := commentToResponse(comment, nil, nil)
	response.IssueRevision = issueRevision
	s.bus.Publish(events.Event{
		Type:        eventType,
		WorkspaceID: uuidToString(comment.WorkspaceID),
		ActorType:   "system",
		ActorID:     uuid.Nil.String(),
		Payload:     map[string]any{"comment": response, "issue_revision": issueRevision},
		TaskID:      uuidToString(comment.IssueID),
	})
}

func (h *Handler) seedLinearOutbound(ctx context.Context, tx pgx.Tx, workspaceID, bindingID, projectID pgtype.UUID) error {
	return linearsync.SeedOutbound(ctx, tx, workspaceID, bindingID, projectID)
}
