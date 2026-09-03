package main

import (
	"context"
	"encoding/json"
	"log/slog"

	"github.com/patchbay-ai/patchbay/server/internal/events"
	"github.com/patchbay-ai/patchbay/server/internal/handler"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

// registerActivityListeners wires up event bus listeners that record activity
// entries in the activity_log table. Each listener creates one or more activity
// records depending on what changed, then publishes an activity:created event
// for WS broadcasting.
func registerActivityListeners(bus *events.Bus, queries *db.Queries) {
	ctx := context.Background()

	// issue:created — record "created" activity
	bus.Subscribe(protocol.EventIssueCreated, func(e events.Event) {
		payload, ok := e.Payload.(map[string]any)
		if !ok {
			return
		}
		issue, ok := payload["issue"].(handler.IssueResponse)
		if !ok {
			return
		}

		activity, err := queries.CreateActivity(ctx, db.CreateActivityParams{
			ID:          dbid.NewV7(),
			WorkspaceID: parseUUID(issue.WorkspaceID),
			IssueID:     parseUUID(issue.ID),
			ActorType:   util.StrToText(e.ActorType),
			ActorID:     optionalUUID(e.ActorID),
			Action:      "created",
			Details:     []byte("{}"),
		})
		if err != nil {
			slog.Error("activity: failed to record issue created",
				"issue_id", issue.ID, "error", err)
			return
		}

		publishActivityEvent(bus, e, activity)
	})

	// issue:updated — record specific changes as separate activities
	bus.Subscribe(protocol.EventIssueUpdated, func(e events.Event) {
		payload, ok := e.Payload.(map[string]any)
		if !ok {
			return
		}
		issue, ok := payload["issue"].(handler.IssueResponse)
		if !ok {
			return
		}

		statusChanged, _ := payload["status_changed"].(bool)
		priorityChanged, _ := payload["priority_changed"].(bool)
		ownerChanged, _ := payload["owner_changed"].(bool)
		executorChanged, _ := payload["executor_changed"].(bool)
		reviewerChanged, _ := payload["reviewer_changed"].(bool)
		reviewHandoff, _ := payload["review_handoff"].(bool)
		descriptionChanged, _ := payload["description_changed"].(bool)

		if reviewHandoff || reviewerChanged {
			previousTypeKey := "prev_reviewer_type"
			previousIDKey := "prev_reviewer_id"
			if reviewHandoff {
				previousTypeKey = "prev_executor_type"
				previousIDKey = "prev_executor_id"
			}
			detailsMap := map[string]string{
				"from_status": payloadString(payload, "prev_status"),
				"to_status":   issue.Status,
			}
			copyOptionalPayloadString(detailsMap, "from_type", payload, previousTypeKey)
			copyOptionalPayloadString(detailsMap, "from_id", payload, previousIDKey)
			if issue.ReviewerType != nil {
				detailsMap["to_type"] = *issue.ReviewerType
			}
			if issue.ReviewerID != nil {
				detailsMap["to_id"] = *issue.ReviewerID
			}
			details, _ := json.Marshal(detailsMap)
			activity, err := queries.CreateActivity(ctx, db.CreateActivityParams{
				ID: dbid.NewV7(), WorkspaceID: parseUUID(issue.WorkspaceID),
				IssueID: parseUUID(issue.ID), ActorType: util.StrToText(e.ActorType),
				ActorID: optionalUUID(e.ActorID), Action: "review_handoff", Details: details,
			})
			if err != nil {
				slog.Error("activity: failed to record review handoff", "issue_id", issue.ID, "error", err)
			} else {
				publishActivityEvent(bus, e, activity)
			}
		}

		if statusChanged && !reviewHandoff {
			prevStatus, _ := payload["prev_status"].(string)
			details, _ := json.Marshal(map[string]string{
				"from": prevStatus,
				"to":   issue.Status,
			})
			activity, err := queries.CreateActivity(ctx, db.CreateActivityParams{
				ID:          dbid.NewV7(),
				WorkspaceID: parseUUID(issue.WorkspaceID),
				IssueID:     parseUUID(issue.ID),
				ActorType:   util.StrToText(e.ActorType),
				ActorID:     optionalUUID(e.ActorID),
				Action:      "status_changed",
				Details:     details,
			})
			if err != nil {
				slog.Error("activity: failed to record status change",
					"issue_id", issue.ID, "error", err)
			} else {
				publishActivityEvent(bus, e, activity)
			}
		}

		if priorityChanged {
			prevPriority, _ := payload["prev_priority"].(string)
			details, _ := json.Marshal(map[string]string{
				"from": prevPriority,
				"to":   issue.Priority,
			})
			activity, err := queries.CreateActivity(ctx, db.CreateActivityParams{
				ID:          dbid.NewV7(),
				WorkspaceID: parseUUID(issue.WorkspaceID),
				IssueID:     parseUUID(issue.ID),
				ActorType:   util.StrToText(e.ActorType),
				ActorID:     optionalUUID(e.ActorID),
				Action:      "priority_changed",
				Details:     details,
			})
			if err != nil {
				slog.Error("activity: failed to record priority change",
					"issue_id", issue.ID, "error", err)
			} else {
				publishActivityEvent(bus, e, activity)
			}
		}

		if executorChanged && !reviewHandoff {
			detailsMap := map[string]string{}
			copyOptionalPayloadString(detailsMap, "from_type", payload, "prev_executor_type")
			copyOptionalPayloadString(detailsMap, "from_id", payload, "prev_executor_id")
			if issue.ExecutorType != nil {
				detailsMap["to_type"] = *issue.ExecutorType
			}
			if issue.ExecutorID != nil {
				detailsMap["to_id"] = *issue.ExecutorID
			}

			details, _ := json.Marshal(detailsMap)
			activity, err := queries.CreateActivity(ctx, db.CreateActivityParams{
				ID:          dbid.NewV7(),
				WorkspaceID: parseUUID(issue.WorkspaceID),
				IssueID:     parseUUID(issue.ID),
				ActorType:   util.StrToText(e.ActorType),
				ActorID:     optionalUUID(e.ActorID),
				Action:      "executor_changed",
				Details:     details,
			})
			if err != nil {
				slog.Error("activity: failed to record executor change",
					"issue_id", issue.ID, "error", err)
			} else {
				publishActivityEvent(bus, e, activity)
			}
		}

		if ownerChanged {
			detailsMap := map[string]string{}
			copyOptionalPayloadString(detailsMap, "from_type", payload, "prev_owner_type")
			copyOptionalPayloadString(detailsMap, "from_id", payload, "prev_owner_id")
			if issue.OwnerType != nil {
				detailsMap["to_type"] = *issue.OwnerType
			}
			if issue.OwnerID != nil {
				detailsMap["to_id"] = *issue.OwnerID
			}
			details, _ := json.Marshal(detailsMap)
			activity, err := queries.CreateActivity(ctx, db.CreateActivityParams{
				ID: dbid.NewV7(), WorkspaceID: parseUUID(issue.WorkspaceID),
				IssueID: parseUUID(issue.ID), ActorType: util.StrToText(e.ActorType),
				ActorID: optionalUUID(e.ActorID), Action: "owner_changed", Details: details,
			})
			if err != nil {
				slog.Error("activity: failed to record owner change", "issue_id", issue.ID, "error", err)
			} else {
				publishActivityEvent(bus, e, activity)
			}
		}

		if startDateChanged, _ := payload["start_date_changed"].(bool); startDateChanged {
			prevStartDate := ""
			if v, ok := payload["prev_start_date"].(*string); ok && v != nil {
				prevStartDate = *v
			}
			newStartDate := ""
			if issue.StartDate != nil {
				newStartDate = *issue.StartDate
			}
			details, _ := json.Marshal(map[string]string{
				"from": prevStartDate,
				"to":   newStartDate,
			})
			activity, err := queries.CreateActivity(ctx, db.CreateActivityParams{
				ID:          dbid.NewV7(),
				WorkspaceID: parseUUID(issue.WorkspaceID),
				IssueID:     parseUUID(issue.ID),
				ActorType:   util.StrToText(e.ActorType),
				ActorID:     optionalUUID(e.ActorID),
				Action:      "start_date_changed",
				Details:     details,
			})
			if err != nil {
				slog.Error("activity: failed to record start date change",
					"issue_id", issue.ID, "error", err)
			} else {
				publishActivityEvent(bus, e, activity)
			}
		}

		if dueDateChanged, _ := payload["due_date_changed"].(bool); dueDateChanged {
			prevDueDate := ""
			if v, ok := payload["prev_due_date"].(*string); ok && v != nil {
				prevDueDate = *v
			}
			newDueDate := ""
			if issue.DueDate != nil {
				newDueDate = *issue.DueDate
			}
			details, _ := json.Marshal(map[string]string{
				"from": prevDueDate,
				"to":   newDueDate,
			})
			activity, err := queries.CreateActivity(ctx, db.CreateActivityParams{
				ID:          dbid.NewV7(),
				WorkspaceID: parseUUID(issue.WorkspaceID),
				IssueID:     parseUUID(issue.ID),
				ActorType:   util.StrToText(e.ActorType),
				ActorID:     optionalUUID(e.ActorID),
				Action:      "due_date_changed",
				Details:     details,
			})
			if err != nil {
				slog.Error("activity: failed to record due date change",
					"issue_id", issue.ID, "error", err)
			} else {
				publishActivityEvent(bus, e, activity)
			}
		}

		if titleChanged, _ := payload["title_changed"].(bool); titleChanged {
			prevTitle, _ := payload["prev_title"].(string)
			details, _ := json.Marshal(map[string]string{
				"from": prevTitle,
				"to":   issue.Title,
			})
			activity, err := queries.CreateActivity(ctx, db.CreateActivityParams{
				ID:          dbid.NewV7(),
				WorkspaceID: parseUUID(issue.WorkspaceID),
				IssueID:     parseUUID(issue.ID),
				ActorType:   util.StrToText(e.ActorType),
				ActorID:     optionalUUID(e.ActorID),
				Action:      "title_changed",
				Details:     details,
			})
			if err != nil {
				slog.Error("activity: failed to record title change",
					"issue_id", issue.ID, "error", err)
			} else {
				publishActivityEvent(bus, e, activity)
			}
		}

		if descriptionChanged {
			activity, err := queries.CreateActivity(ctx, db.CreateActivityParams{
				ID:          dbid.NewV7(),
				WorkspaceID: parseUUID(issue.WorkspaceID),
				IssueID:     parseUUID(issue.ID),
				ActorType:   util.StrToText(e.ActorType),
				ActorID:     optionalUUID(e.ActorID),
				Action:      "description_updated",
				Details:     []byte("{}"),
			})
			if err != nil {
				slog.Error("activity: failed to record description change",
					"issue_id", issue.ID, "error", err)
			} else {
				publishActivityEvent(bus, e, activity)
			}
		}
	})

	// task:completed — record "task_completed" activity
	bus.Subscribe(protocol.EventTaskCompleted, func(e events.Event) {
		handleTaskActivity(ctx, bus, queries, e, "task_completed")
	})

	// task:failed — record "task_failed" activity
	bus.Subscribe(protocol.EventTaskFailed, func(e events.Event) {
		handleTaskActivity(ctx, bus, queries, e, "task_failed")
	})
}

// handleTaskActivity records an activity for task:completed or task:failed events.
func handleTaskActivity(ctx context.Context, bus *events.Bus, queries *db.Queries, e events.Event, action string) {
	payload, ok := e.Payload.(map[string]any)
	if !ok {
		return
	}
	agentID, _ := payload["agent_id"].(string)
	issueID, _ := payload["issue_id"].(string)
	if issueID == "" {
		return
	}

	// Look up issue to get workspace_id
	issue, err := queries.GetIssue(ctx, parseUUID(issueID))
	if err != nil {
		slog.Error("activity: failed to get issue for task event",
			"issue_id", issueID, "action", action, "error", err)
		return
	}

	activity, err := queries.CreateActivity(ctx, db.CreateActivityParams{
		ID:          dbid.NewV7(),
		WorkspaceID: issue.WorkspaceID,
		IssueID:     parseUUID(issueID),
		ActorType:   util.StrToText("agent"),
		ActorID:     parseUUID(agentID),
		Action:      action,
		Details:     []byte("{}"),
	})
	if err != nil {
		slog.Error("activity: failed to record task activity",
			"issue_id", issueID, "action", action, "error", err)
		return
	}

	publishActivityEvent(bus, e, activity)
}

func payloadString(payload map[string]any, key string) string {
	switch value := payload[key].(type) {
	case string:
		return value
	case *string:
		if value != nil {
			return *value
		}
	}
	return ""
}

func copyOptionalPayloadString(target map[string]string, targetKey string, payload map[string]any, sourceKey string) {
	if value := payloadString(payload, sourceKey); value != "" {
		target[targetKey] = value
	}
}

// publishActivityEvent sends an activity:created event for WS broadcasting.
// Payload matches frontend ActivityCreatedPayload: { issue_id, entry: TimelineEntry }
func publishActivityEvent(bus *events.Bus, original events.Event, activity db.ActivityLog) {
	actorType := ""
	if activity.ActorType.Valid {
		actorType = activity.ActorType.String
	}
	action := activity.Action
	bus.Publish(events.Event{
		Type:        protocol.EventActivityCreated,
		WorkspaceID: original.WorkspaceID,
		ActorType:   original.ActorType,
		ActorID:     original.ActorID,
		Payload: map[string]any{
			"issue_id": util.UUIDToString(activity.IssueID),
			"entry": map[string]any{
				"type":       "activity",
				"id":         util.UUIDToString(activity.ID),
				"actor_type": actorType,
				"actor_id":   util.UUIDToString(activity.ActorID),
				"action":     action,
				"details":    json.RawMessage(activity.Details),
				"created_at": util.TimestampToString(activity.CreatedAt),
			},
		},
	})
}
