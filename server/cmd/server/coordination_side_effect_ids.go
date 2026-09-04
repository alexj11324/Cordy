package main

import (
	"fmt"
	"strings"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/events"
)

// durableCoordinationID returns a stable UUID for a side effect of one
// coordinator publication. The publication event id is the retry boundary;
// scope and parts keep independent activities/inbox deliveries distinct.
// Ordinary events deliberately return no stable id and continue using UUIDv7.
func durableCoordinationID(event events.Event, scope string, parts ...string) (pgtype.UUID, bool) {
	payload, ok := event.Payload.(map[string]any)
	if !ok {
		return pgtype.UUID{}, false
	}
	eventID, ok := payload["coordination_event_id"].(string)
	if !ok || strings.TrimSpace(eventID) == "" {
		return pgtype.UUID{}, false
	}
	publication, _ := payload["coordination_publication"].(string)
	if publication == "" {
		if reviewHandoff, _ := payload["review_handoff"].(bool); reviewHandoff {
			publication = "review_handoff"
		} else if reviewerChanged, _ := payload["reviewer_changed"].(bool); reviewerChanged {
			publication = "reviewer_replacement"
		} else {
			return pgtype.UUID{}, false
		}
	}
	if publication != "review_handoff" && publication != "reviewer_replacement" && publication != "assignment_activity" {
		return pgtype.UUID{}, false
	}
	name := fmt.Sprintf("patchbay:coordination:%s:%s:%s:%s", scope, eventID, publication, strings.Join(parts, ":"))
	id := uuid.NewSHA1(uuid.NameSpaceOID, []byte(name))
	return pgtype.UUID{Bytes: id, Valid: true}, true
}
