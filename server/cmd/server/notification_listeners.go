package main

import (
	"context"
	"encoding/json"
	"log/slog"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/events"
	"github.com/patchbay-ai/patchbay/server/internal/handler"
	"github.com/patchbay-ai/patchbay/server/internal/issuestatus"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

// mention represents a parsed @mention from markdown content (local alias).
type mention struct {
	Type string // "member", "agent", "issue", or "all"
	ID   string // user_id, agent_id, issue_id, or "all"
}

// statusLabels maps DB status values to human-readable labels for notifications.
var statusLabels = map[string]string{
	"backlog":     "Backlog",
	"todo":        "Todo",
	"in_progress": "In Progress",
	"in_review":   "In Review",
	"done":        "Done",
	"blocked":     "Blocked",
	"cancelled":   "Cancelled",
}

// priorityLabels maps DB priority values to human-readable labels for notifications.
var priorityLabels = map[string]string{
	"urgent": "Urgent",
	"high":   "High",
	"medium": "Medium",
	"low":    "Low",
	"none":   "No priority",
}

func statusLabel(s string) string {
	if l, ok := statusLabels[s]; ok {
		return l
	}
	return s
}

func priorityLabel(p string) string {
	if l, ok := priorityLabels[p]; ok {
		return l
	}
	return p
}

var emptyDetails = []byte("{}")

// parseMentions extracts mentions from markdown content.
// Delegates to the shared util.ParseMentions and converts to the local type.
func parseMentions(content string) []mention {
	parsed := util.ParseMentions(content)
	result := make([]mention, len(parsed))
	for i, m := range parsed {
		result[i] = mention{Type: m.Type, ID: m.ID}
	}
	return result
}

// parentBubbleNotifTypes is the allowlist of inbox notification types that
// bubble up from a sub-issue to subscribers of its parent. Other event types
// only notify subscribers of the sub-issue itself, to keep parent watchers'
// inboxes focused on the signal that matters most: status transitions.
var parentBubbleNotifTypes = map[string]bool{
	"status_changed": true,
}

// delegatedAlwaysNotifTypes are the events a DELEGATED subscriber (reason=
// 'delegated' — an agent created this issue on their behalf, MUL-5483) receives
// unconditionally: they are either addressed at the human directly, or they are
// exceptions that stall the work until a human looks.
var delegatedAlwaysNotifTypes = map[string]bool{
	"mentioned":     true,
	"task_failed":   true,
	"agent_blocked": true,
}

// delegatedStatusNotify are the statuses whose ARRIVAL is worth an inbox item
// for a delegated subscriber — someone whose agent filed this issue on their
// behalf.
//
// in_review is the important one: in Patchbay's agent flow an agent parks
// completed work in in_review, so that is the dominant "this needs you now"
// transition, not done.
//
// Everything absent from this set is churn for someone who never asked for this
// specific issue: routine forward progress (todo, in_progress), deliberate
// parking (backlog), comment traffic, and field edits.
var delegatedStatusNotify = map[string]bool{
	"in_review": true,
	"done":      true,
	"cancelled": true,
	"blocked":   true,
}

// deliverToSubscriber reports whether a subscriber row should receive this
// notification type. Direct subscriptions (creator / owner / executor / commenter /
// mentioned / manual / automation) are unchanged — they opted in to this issue,
// explicitly or by acting on it. Only the delegated tier is narrowed, and only
// to drop churn.
//
// A child finishing is NOT churn: "sub-issue X is ready for review" is exactly
// the signal a delegated watcher needs, and there is one per piece of real work.
// An earlier cut suppressed those and synthesized a single "the whole batch
// finished" roll-up from sibling state instead. That was both the wrong shape
// (see the MUL-5483 thread) and redundant: the human is subscribed to the PARENT
// as well, so when the agent moves the parent to in_review/done that transition
// delivers here — which is the natural "the tree is done" signal. Deriving it
// from children was reinventing a notification the platform already sends.
func deliverToSubscriber(reason, notifType, issueStatus string) bool {
	if reason != "delegated" {
		return true
	}
	if delegatedAlwaysNotifTypes[notifType] {
		return true
	}
	if notifType != "status_changed" {
		return false
	}
	return delegatedStatusNotify[issueStatus]
}

// notifTypeToGroup maps each InboxItemType to a user-configurable preference
// group. Types not in this map are always delivered (not configurable).
// `issue_assigned` and `unassigned` are retained wire values for the excluded
// frontend/inbox clients; their details payloads carry the explicit owner,
// executor, or reviewer keys below.
var notifTypeToGroup = map[string]string{
	"issue_assigned":     "assignments",
	"unassigned":         "assignments",
	"owner_changed":      "assignments",
	"executor_changed":   "assignments",
	"review_requested":   "assignments",
	"status_changed":     "status_changes",
	"new_comment":        "comments",
	"mentioned":          "mentions",
	"priority_changed":   "updates",
	"start_date_changed": "updates",
	"due_date_changed":   "updates",
	"task_completed":     "agent_activity",
	"task_failed":        "agent_activity",
	"agent_blocked":      "agent_activity",
	"agent_completed":    "agent_activity",
}

// isNotifMuted returns true if the given notification type is muted for a user
// based on their parsed preferences map.
func isNotifMuted(prefs map[string]string, notifType string) bool {
	group, ok := notifTypeToGroup[notifType]
	if !ok {
		return false // unconfigurable types are always delivered
	}
	return prefs[group] == "muted"
}

// loadUserPrefs loads notification preferences for a set of user IDs in a
// workspace. Returns a map from user_id string to parsed preferences.
func loadUserPrefs(
	ctx context.Context,
	queries *db.Queries,
	workspaceID string,
	userIDs []string,
) map[string]map[string]string {
	if len(userIDs) == 0 {
		return nil
	}

	uuids := make([]pgtype.UUID, len(userIDs))
	for i, id := range userIDs {
		uuids[i] = parseUUID(id)
	}

	rows, err := queries.ListNotificationPreferencesByUsers(ctx, db.ListNotificationPreferencesByUsersParams{
		WorkspaceID: parseUUID(workspaceID),
		UserIds:     uuids,
	})
	if err != nil {
		slog.Error("failed to load notification preferences", "error", err)
		return nil
	}

	result := make(map[string]map[string]string, len(rows))
	for _, row := range rows {
		var prefs map[string]string
		if err := json.Unmarshal(row.Preferences, &prefs); err != nil {
			continue
		}
		result[util.UUIDToString(row.UserID)] = prefs
	}
	return result
}

// terminalStatusForTaskFailedDismiss is the set of issue statuses that mark
// the issue as "the user no longer needs to triage past failures." When a
// status change lands on one of these, any pre-existing task_failed inbox
// rows for the issue are archived so the inbox stays a fresh-signal surface.
// `in_review` is included because in Patchbay's agent flow that's the most
// reliable "work delivered" handoff — and a status flip back to in_progress
// will simply produce new task_failed rows that surface normally.
var terminalStatusForTaskFailedDismiss = map[string]bool{
	"in_review": true,
	"done":      true,
	"cancelled": true,
}

// archiveStaleTaskFailedInbox archives all task_failed inbox rows for the
// given issue and notifies each affected member recipient via
// inbox:batch-archived so connected clients self-heal.
func archiveStaleTaskFailedInbox(
	ctx context.Context,
	queries *db.Queries,
	bus *events.Bus,
	workspaceID string,
	issueID string,
) {
	rows, err := queries.ArchiveInboxByIssueAndType(ctx, db.ArchiveInboxByIssueAndTypeParams{
		WorkspaceID: parseUUID(workspaceID),
		IssueID:     parseUUID(issueID),
		Type:        "task_failed",
	})
	if err != nil {
		slog.Error("auto-archive task_failed inbox: query failed",
			"workspace_id", workspaceID, "issue_id", issueID, "error", err)
		return
	}
	if len(rows) == 0 {
		return
	}

	// Dedupe recipients: the listener creates one row per failure event per
	// subscriber, so a long-running issue can yield several rows for the
	// same recipient.
	counts := map[string]int{}
	for _, row := range rows {
		// Inbox rows for task_failed only target member recipients today
		// (notifySubscribers skips agent subscribers), but defend the WS
		// layer against future widening — only members get a personal feed.
		if row.RecipientType != "member" {
			continue
		}
		counts[util.UUIDToString(row.RecipientID)]++
	}

	for recipientID, count := range counts {
		bus.Publish(events.Event{
			Type:        protocol.EventInboxBatchArchived,
			WorkspaceID: workspaceID,
			Payload: map[string]any{
				"recipient_id": recipientID,
				"count":        int64(count),
				"issue_id":     issueID,
				"reason":       "issue_status_terminal",
			},
		})
	}

	slog.Info("auto-archive task_failed inbox: archived stale rows",
		"workspace_id", workspaceID, "issue_id", issueID,
		"row_count", len(rows), "recipient_count", len(counts))
}

// notifySubscribers queries the subscriber table for an issue, excludes the
// actor and any extra IDs, and creates inbox items for each remaining member
// subscriber. Publishes an inbox:new event for each notification.
// If the issue has a parent and the notification type is in the bubble
// allowlist, parent issue subscribers are also notified (deduplicated
// against direct subscribers).
func notifySubscribers(
	ctx context.Context,
	queries *db.Queries,
	bus *events.Bus,
	issueID string,
	issueStatus string,
	workspaceID string,
	e events.Event,
	exclude map[string]bool,
	notifType string,
	severity string,
	title string,
	body string,
	details []byte,
) {
	notified, tierSuppressed := notifyIssueSubscribers(ctx, queries, bus,
		issueID, issueID, issueStatus, workspaceID, e, exclude,
		notifType, severity, title, body, details)

	// Only a small allowlist of event types bubbles to parent subscribers.
	if !parentBubbleNotifTypes[notifType] {
		return
	}

	// Also notify parent issue subscribers if this is a sub-issue.
	issue, err := queries.GetIssue(ctx, parseUUID(issueID))
	if err != nil {
		slog.Error("failed to get issue for parent notification",
			"issue_id", issueID, "error", err)
		return
	}
	if !issue.ParentIssueID.Valid {
		return
	}

	// Merge already-notified IDs into exclude set for parent subscribers.
	parentExclude := make(map[string]bool, len(exclude)+len(notified)+len(tierSuppressed))
	for id := range exclude {
		parentExclude[id] = true
	}
	for id := range notified {
		parentExclude[id] = true
	}
	// Recipients the delegated tier just filtered out for THIS child must not
	// get the same event smuggled back in through the parent's subscriber list.
	// The common shape is exactly that: the human directly created the parent
	// (reason='creator', full delivery) while their agent filed the children
	// (reason='delegated', reduced). Without this the tier suppresses nothing
	// in the one case it exists for — an agent-built tree under a parent the
	// human is watching (MUL-5483).
	for id := range tierSuppressed {
		parentExclude[id] = true
	}

	// Query subscribers from the parent issue, but the inbox item still
	// points to the sub-issue so the user navigates to the actual change.
	parentID := util.UUIDToString(issue.ParentIssueID)
	notifyIssueSubscribers(ctx, queries, bus,
		parentID, issueID, issueStatus, workspaceID, e, parentExclude,
		notifType, severity, title, body, details)
}

// notifyIssueSubscribers sends inbox notifications to subscribers of
// subscriberIssueID, but creates inbox items pointing to targetIssueID.
// This allows querying subscribers from a parent issue while the notification
// links to the sub-issue where the change actually occurred.
//
// Returns two sets of member IDs: those that were notified, and those the
// delegated delivery tier deliberately filtered out. The caller propagates the
// second set into the parent bubble so a suppressed event cannot be
// re-delivered through an ancestor subscription (see notifySubscribers).
func notifyIssueSubscribers(
	ctx context.Context,
	queries *db.Queries,
	bus *events.Bus,
	subscriberIssueID string,
	targetIssueID string,
	issueStatus string,
	workspaceID string,
	e events.Event,
	exclude map[string]bool,
	notifType string,
	severity string,
	title string,
	body string,
	details []byte,
) (map[string]bool, map[string]bool) {
	notified := map[string]bool{}
	tierSuppressed := map[string]bool{}

	// Normalize a custom status to the canonical status it inherits, so the
	// delegated tier's status allowlist below keys off behavior rather than a
	// literal. A built-in key returns itself without a query, so the common
	// path is unchanged. (MUL-6243)
	issueStatus = issuestatus.Effective(ctx, queries, parseUUID(workspaceID), issueStatus)

	subs, err := queries.ListIssueSubscribers(ctx, parseUUID(subscriberIssueID))
	if err != nil {
		slog.Error("failed to list subscribers for notification",
			"issue_id", subscriberIssueID, "error", err)
		return notified, tierSuppressed
	}

	// Batch-load notification preferences for all member subscribers.
	var memberIDs []string
	for _, sub := range subs {
		if sub.UserType == "member" {
			memberIDs = append(memberIDs, util.UUIDToString(sub.UserID))
		}
	}
	userPrefs := loadUserPrefs(ctx, queries, workspaceID, memberIDs)

	for _, sub := range subs {
		// Only notify member-type subscribers (not agents)
		if sub.UserType != "member" {
			continue
		}

		subID := util.UUIDToString(sub.UserID)

		// Skip the actor
		if subID == e.ActorID {
			continue
		}

		// Skip any extra excluded IDs
		if exclude[subID] {
			continue
		}

		// Skip if this notification type is muted by the user
		if prefs, ok := userPrefs[subID]; ok && isNotifMuted(prefs, notifType) {
			continue
		}

		// Delegated subscriptions deliver a narrower event set than direct
		// ones — see deliverToSubscriber.
		if !deliverToSubscriber(sub.Reason, notifType, issueStatus) {
			tierSuppressed[subID] = true
			continue
		}

		item, err := queries.CreateInboxItem(ctx, db.CreateInboxItemParams{
			ID:            dbid.NewV7(),
			WorkspaceID:   parseUUID(workspaceID),
			RecipientType: "member",
			RecipientID:   sub.UserID,
			Type:          notifType,
			Severity:      severity,
			IssueID:       parseUUID(targetIssueID),
			Title:         title,
			Body:          util.StrToText(body),
			ActorType:     util.StrToText(e.ActorType),
			ActorID:       optionalUUID(e.ActorID),
			Details:       details,
		})
		if err != nil {
			slog.Error("subscriber notification creation failed",
				"subscriber_id", subID, "type", notifType, "error", err)
			continue
		}

		notified[subID] = true
		resp := inboxItemToResponse(item)
		resp["issue_status"] = issueStatus
		bus.Publish(events.Event{
			Type:        protocol.EventInboxNew,
			WorkspaceID: workspaceID,
			ActorType:   e.ActorType,
			ActorID:     e.ActorID,
			Payload:     map[string]any{"item": resp},
		})
	}

	return notified, tierSuppressed
}

// notifyDirect creates an inbox item for a specific recipient. Skips if the
// recipient is the actor. Publishes an inbox:new event on success.
func notifyDirect(
	ctx context.Context,
	queries *db.Queries,
	bus *events.Bus,
	recipientType string,
	recipientID string,
	workspaceID string,
	e events.Event,
	issueID string,
	issueStatus string,
	notifType string,
	severity string,
	title string,
	body string,
	details []byte,
) {
	// Skip if recipient is the actor
	if recipientID == e.ActorID {
		return
	}

	// Check notification preferences for member recipients.
	if recipientType == "member" {
		prefs := loadUserPrefs(ctx, queries, workspaceID, []string{recipientID})
		if p, ok := prefs[recipientID]; ok && isNotifMuted(p, notifType) {
			return
		}
	}

	item, err := queries.CreateInboxItem(ctx, db.CreateInboxItemParams{
		ID:            dbid.NewV7(),
		WorkspaceID:   parseUUID(workspaceID),
		RecipientType: recipientType,
		RecipientID:   parseUUID(recipientID),
		Type:          notifType,
		Severity:      severity,
		IssueID:       parseUUID(issueID),
		Title:         title,
		Body:          util.StrToText(body),
		ActorType:     util.StrToText(e.ActorType),
		ActorID:       optionalUUID(e.ActorID),
		Details:       details,
	})
	if err != nil {
		slog.Error("direct notification creation failed",
			"issue_id", issueID, "recipient_id", recipientID, "type", notifType, "error", err)
		return
	}

	resp := inboxItemToResponse(item)
	resp["issue_status"] = issueStatus
	bus.Publish(events.Event{
		Type:        protocol.EventInboxNew,
		WorkspaceID: workspaceID,
		ActorType:   e.ActorType,
		ActorID:     e.ActorID,
		Payload:     map[string]any{"item": resp},
	})
}

// notifyMentionedMembers creates inbox items for each @mentioned member,
// excluding the actor and any IDs in the skip set. When an @all mention is
// present, all workspace members are notified (excluding agents).
func notifyMentionedMembers(
	bus *events.Bus,
	queries *db.Queries,
	e events.Event,
	mentions []mention,
	issueID string,
	issueTitle string,
	issueStatus string,
	title string,
	skip map[string]bool,
	details []byte,
) {
	// Collect the set of member IDs to notify.
	recipientIDs := map[string]bool{}

	hasAll := false
	var teamIDs []string
	for _, m := range mentions {
		if m.Type == "all" {
			hasAll = true
			continue
		}
		if m.Type == "member" {
			recipientIDs[m.ID] = true
		}
		if m.Type == "team" {
			teamIDs = append(teamIDs, m.ID)
		}
	}

	// Expand each @team mention to its human members. Agent members of a
	// team are reached via comment-trigger / executor-routing paths, not the
	// mention-inbox path, so we only seed member-typed recipients here.
	for _, sid := range teamIDs {
		teamUUID, err := util.ParseUUID(sid)
		if err != nil {
			continue
		}
		members, err := queries.ListTeamMembers(context.Background(), teamUUID)
		if err != nil {
			slog.Error("failed to list team members for @team mention", "team_id", sid, "error", err)
			continue
		}
		for _, sm := range members {
			if sm.MemberType == "member" {
				recipientIDs[util.UUIDToString(sm.MemberID)] = true
			}
		}
	}

	// If @all is present, expand to all workspace members.
	if hasAll {
		members, err := queries.ListMembers(context.Background(), parseUUID(e.WorkspaceID))
		if err != nil {
			slog.Error("failed to list members for @all mention", "workspace_id", e.WorkspaceID, "error", err)
		} else {
			for _, m := range members {
				recipientIDs[util.UUIDToString(m.UserID)] = true
			}
		}
	}

	// Batch-load notification preferences for all mention recipients.
	var mentionUserIDs []string
	for id := range recipientIDs {
		if id != e.ActorID && !skip[id] {
			mentionUserIDs = append(mentionUserIDs, id)
		}
	}
	mentionPrefs := loadUserPrefs(context.Background(), queries, e.WorkspaceID, mentionUserIDs)

	for id := range recipientIDs {
		if id == e.ActorID || skip[id] {
			continue
		}
		// Skip if mentions are muted by this user. This is deliberately a
		// different group from `comments`: muting comment volume must not
		// silence someone asking for you by name.
		if p, ok := mentionPrefs[id]; ok && isNotifMuted(p, "mentioned") {
			continue
		}
		item, err := queries.CreateInboxItem(context.Background(), db.CreateInboxItemParams{
			ID:            dbid.NewV7(),
			WorkspaceID:   parseUUID(e.WorkspaceID),
			RecipientType: "member",
			RecipientID:   parseUUID(id),
			Type:          "mentioned",
			Severity:      "info",
			IssueID:       parseUUID(issueID),
			Title:         title,
			ActorType:     util.StrToText(e.ActorType),
			ActorID:       optionalUUID(e.ActorID),
			Details:       details,
		})
		if err != nil {
			slog.Error("mention inbox creation failed", "mentioned_id", id, "error", err)
			continue
		}
		resp := inboxItemToResponse(item)
		resp["issue_status"] = issueStatus
		bus.Publish(events.Event{
			Type:        protocol.EventInboxNew,
			WorkspaceID: e.WorkspaceID,
			ActorType:   e.ActorType,
			ActorID:     e.ActorID,
			Payload:     map[string]any{"item": resp},
		})
	}
}

// registerNotificationListeners wires up event bus listeners that create inbox
// notifications using the subscriber table. This replaces the old hardcoded
// notification logic from inbox_listeners.go.
//
// NOTE: uses context.Background() because the event bus dispatches synchronously
// within the HTTP request goroutine. Adding per-handler timeouts is a bus-level
// concern — see events.Bus for future improvements.
func registerNotificationListeners(bus *events.Bus, queries *db.Queries) {
	ctx := context.Background()

	// issue:created — direct notifications to owner/executor when they differ from the actor
	bus.Subscribe(protocol.EventIssueCreated, func(e events.Event) {
		payload, ok := e.Payload.(map[string]any)
		if !ok {
			return
		}
		issue, ok := payload["issue"].(handler.IssueResponse)
		if !ok {
			return
		}

		// Track who already got notified to avoid duplicates
		skip := map[string]bool{e.ActorID: true}

		// Direct notification to an owner that owns an inbox.
		if issue.OwnerType != nil && issue.OwnerID != nil && isIssueRoleRecipientType(*issue.OwnerType) {
			skip[*issue.OwnerID] = true
			details, _ := json.Marshal(map[string]string{
				"new_owner_type": *issue.OwnerType,
				"new_owner_id":   *issue.OwnerID,
			})
			notifyDirect(ctx, queries, bus,
				*issue.OwnerType, *issue.OwnerID,
				issue.WorkspaceID, e, issue.ID, issue.Status,
				"issue_assigned", "action_required",
				issue.Title,
				"",
				details,
			)
		}

		// Direct notification to an executor that owns an inbox.
		if issue.ExecutorType != nil && issue.ExecutorID != nil && isIssueRoleRecipientType(*issue.ExecutorType) {
			skip[*issue.ExecutorID] = true
			details, _ := json.Marshal(map[string]string{
				"new_executor_type": *issue.ExecutorType,
				"new_executor_id":   *issue.ExecutorID,
			})
			notifyDirect(ctx, queries, bus,
				*issue.ExecutorType, *issue.ExecutorID,
				issue.WorkspaceID, e, issue.ID, issue.Status,
				"issue_assigned", "action_required",
				issue.Title,
				"",
				details,
			)
		}

		if issue.ReviewerType != nil && issue.ReviewerID != nil && isIssueRoleRecipientType(*issue.ReviewerType) {
			skip[*issue.ReviewerID] = true
			details, _ := json.Marshal(map[string]string{
				"new_reviewer_type": *issue.ReviewerType,
				"new_reviewer_id":   *issue.ReviewerID,
			})
			notifyDirect(ctx, queries, bus,
				*issue.ReviewerType, *issue.ReviewerID,
				issue.WorkspaceID, e, issue.ID, issue.Status,
				"review_requested", "action_required",
				issue.Title,
				"",
				details,
			)
		}

		// Notify @mentions in description
		if issue.Description != nil && *issue.Description != "" {
			mentions := parseMentions(*issue.Description)
			notifyMentionedMembers(bus, queries, e, mentions, issue.ID, issue.Title, issue.Status,
				issue.Title, skip, emptyDetails)
		}
	})

	// issue:updated — handle role changes, status changes, priority, due date
	bus.Subscribe(protocol.EventIssueUpdated, func(e events.Event) {
		payload, ok := e.Payload.(map[string]any)
		if !ok {
			return
		}
		issue, ok := payload["issue"].(handler.IssueResponse)
		if !ok {
			return
		}
		ownerChanged, _ := payload["owner_changed"].(bool)
		executorChanged, _ := payload["executor_changed"].(bool)
		reviewerChanged, _ := payload["reviewer_changed"].(bool)
		reviewHandoff, _ := payload["review_handoff"].(bool)
		statusChanged, _ := payload["status_changed"].(bool)
		descriptionChanged, _ := payload["description_changed"].(bool)
		prevDescription, _ := payload["prev_description"].(*string)

		notifyRoleChange := func(
			role string,
			changed bool,
			prevType *string,
			prevID *string,
			newType *string,
			newID *string,
		) {
			if !changed {
				return
			}

			detailsMap := map[string]string{}
			if prevType != nil {
				detailsMap["prev_"+role+"_type"] = *prevType
			}
			if prevID != nil {
				detailsMap["prev_"+role+"_id"] = *prevID
			}
			if newType != nil {
				detailsMap["new_"+role+"_type"] = *newType
			}
			if newID != nil {
				detailsMap["new_"+role+"_id"] = *newID
			}
			roleDetails, _ := json.Marshal(detailsMap)

			// Direct: notify the new role actor when it owns an inbox.
			if newType != nil && newID != nil && isIssueRoleRecipientType(*newType) {
				notifyDirect(ctx, queries, bus,
					*newType, *newID,
					e.WorkspaceID, e, issue.ID, issue.Status,
					"issue_assigned", "action_required",
					issue.Title,
					"",
					roleDetails,
				)
			}

			// Direct: notify only a previous member about unassignment.
			// This is intentionally narrower than isIssueRoleRecipientType: agents
			// do not receive unassigned notifications.
			if prevType != nil && prevID != nil && *prevType == "member" {
				notifyDirect(ctx, queries, bus,
					"member", *prevID,
					e.WorkspaceID, e, issue.ID, issue.Status,
					"unassigned", "info",
					issue.Title,
					"",
					roleDetails,
				)
			}

			// Subscriber: notify remaining subscribers about the role change,
			// excluding actor, prior role actor, and new role actor.
			exclude := map[string]bool{}
			if prevID != nil {
				exclude[*prevID] = true
			}
			if newID != nil {
				exclude[*newID] = true
			}
			notifySubscribers(ctx, queries, bus, issue.ID, issue.Status, e.WorkspaceID, e,
				exclude, role+"_changed", "info",
				issue.Title, "",
				roleDetails)
		}

		prevOwnerType, _ := payload["prev_owner_type"].(*string)
		prevOwnerID, _ := payload["prev_owner_id"].(*string)
		notifyRoleChange("owner", ownerChanged, prevOwnerType, prevOwnerID, issue.OwnerType, issue.OwnerID)

		prevExecutorType, _ := payload["prev_executor_type"].(*string)
		prevExecutorID, _ := payload["prev_executor_id"].(*string)
		notifyRoleChange("executor", executorChanged, prevExecutorType, prevExecutorID, issue.ExecutorType, issue.ExecutorID)

		if reviewerChanged || reviewHandoff {
			if issue.ReviewerType != nil && issue.ReviewerID != nil && isIssueRoleRecipientType(*issue.ReviewerType) {
				details, _ := json.Marshal(map[string]string{
					"new_reviewer_type": *issue.ReviewerType,
					"new_reviewer_id":   *issue.ReviewerID,
				})
				notifyDirect(ctx, queries, bus,
					*issue.ReviewerType, *issue.ReviewerID,
					e.WorkspaceID, e, issue.ID, issue.Status,
					"review_requested", "action_required",
					issue.Title,
					"",
					details,
				)
			}
		}

		if statusChanged {
			prevStatus, _ := payload["prev_status"].(string)
			statusDetails, _ := json.Marshal(map[string]string{
				"from": prevStatus,
				"to":   issue.Status,
			})
			notifySubscribers(ctx, queries, bus, issue.ID, issue.Status, e.WorkspaceID, e,
				nil, "status_changed", "info",
				issue.Title, "",
				statusDetails)

			// When the issue progresses past the failure (in_review / done /
			// cancelled), retire any stale task_failed inbox rows so the
			// inbox reflects the current state of the work, not its history.
			// The activity log keeps the full failure history for audit.
			if terminalStatusForTaskFailedDismiss[issuestatus.Effective(
				ctx, queries, parseUUID(e.WorkspaceID), issue.Status,
			)] {
				archiveStaleTaskFailedInbox(ctx, queries, bus, e.WorkspaceID, issue.ID)
			}
		}

		if priorityChanged, _ := payload["priority_changed"].(bool); priorityChanged {
			prevPriority, _ := payload["prev_priority"].(string)
			priorityDetails, _ := json.Marshal(map[string]string{
				"from": prevPriority,
				"to":   issue.Priority,
			})
			notifySubscribers(ctx, queries, bus, issue.ID, issue.Status, e.WorkspaceID, e,
				nil, "priority_changed", "info",
				issue.Title, "",
				priorityDetails)
		}

		if startDateChanged, _ := payload["start_date_changed"].(bool); startDateChanged {
			prevStartDateStr := ""
			if prevStartDate, ok := payload["prev_start_date"].(*string); ok && prevStartDate != nil {
				prevStartDateStr = *prevStartDate
			}
			newStartDateStr := ""
			if issue.StartDate != nil {
				newStartDateStr = *issue.StartDate
			}
			startDateDetails, _ := json.Marshal(map[string]string{
				"from": prevStartDateStr,
				"to":   newStartDateStr,
			})
			notifySubscribers(ctx, queries, bus, issue.ID, issue.Status, e.WorkspaceID, e,
				nil, "start_date_changed", "info",
				issue.Title, "",
				startDateDetails)
		}

		if dueDateChanged, _ := payload["due_date_changed"].(bool); dueDateChanged {
			prevDueDateStr := ""
			if prevDueDate, ok := payload["prev_due_date"].(*string); ok && prevDueDate != nil {
				prevDueDateStr = *prevDueDate
			}
			newDueDateStr := ""
			if issue.DueDate != nil {
				newDueDateStr = *issue.DueDate
			}
			dueDateDetails, _ := json.Marshal(map[string]string{
				"from": prevDueDateStr,
				"to":   newDueDateStr,
			})
			notifySubscribers(ctx, queries, bus, issue.ID, issue.Status, e.WorkspaceID, e,
				nil, "due_date_changed", "info",
				issue.Title, "",
				dueDateDetails)
		}

		// Notify NEW @mentions in description
		if descriptionChanged && issue.Description != nil {
			newMentions := parseMentions(*issue.Description)
			if len(newMentions) > 0 {
				prevMentioned := map[string]bool{}
				if prevDescription != nil {
					for _, m := range parseMentions(*prevDescription) {
						prevMentioned[m.Type+":"+m.ID] = true
					}
				}
				var added []mention
				for _, m := range newMentions {
					if !prevMentioned[m.Type+":"+m.ID] {
						added = append(added, m)
					}
				}
				skip := map[string]bool{e.ActorID: true}
				notifyMentionedMembers(bus, queries, e, added, issue.ID, issue.Title, issue.Status,
					issue.Title, skip, emptyDetails)
			}
		}
	})

	// comment:created — notify all subscribers except the commenter
	bus.Subscribe(protocol.EventCommentCreated, func(e events.Event) {
		payload, ok := e.Payload.(map[string]any)
		if !ok {
			return
		}

		// The comment payload can come as handler.CommentResponse from the
		// HTTP handler, or as map[string]any from the agent comment path in
		// task.go. Handle both.
		var issueID, commentID, commentContent, authorType string
		switch c := payload["comment"].(type) {
		case handler.CommentResponse:
			issueID = c.IssueID
			commentID = c.ID
			commentContent = c.Content
			authorType = c.AuthorType
		case map[string]any:
			issueID, _ = c["issue_id"].(string)
			commentID, _ = c["id"].(string)
			commentContent, _ = c["content"].(string)
			authorType, _ = c["author_type"].(string)
		default:
			return
		}

		// Platform-authored system comments (MUL-2538 child-done parent
		// notify) must NOT create inbox rows or parse mentions from their
		// body — the comment is a controlled platform signal, not a human
		// commenter. Mention parsing is the dangerous bit: if the body
		// transcluded a child title containing `mention://member/<uuid>`,
		// a parent's role actor inbox would light up via the generic path.
		// Skip the listener entirely; the WS broadcast still delivers the
		// comment to the issue timeline.
		if authorType == "system" {
			return
		}

		issueTitle, _ := payload["issue_title"].(string)
		issueStatus, _ := payload["issue_status"].(string)

		commentDetails := emptyDetails
		if commentID != "" {
			commentDetails, _ = json.Marshal(map[string]string{
				"comment_id": commentID,
			})
		}

		notifySubscribers(ctx, queries, bus, issueID, issueStatus, e.WorkspaceID, e,
			nil, "new_comment", "info",
			issueTitle, commentContent,
			commentDetails)

		// Notify @mentions in comment content.
		mentions := parseMentions(commentContent)
		if len(mentions) > 0 {
			skip := map[string]bool{e.ActorID: true}
			notifyMentionedMembers(bus, queries, e, mentions, issueID, issueTitle, issueStatus,
				issueTitle, skip, commentDetails)
		}
	})

	// issue_reaction:added — notify the issue creator
	bus.Subscribe(protocol.EventIssueReactionAdded, func(e events.Event) {
		payload, ok := e.Payload.(map[string]any)
		if !ok {
			return
		}

		reaction, ok := payload["reaction"].(handler.IssueReactionResponse)
		if !ok {
			return
		}

		creatorType, _ := payload["creator_type"].(string)
		creatorID, _ := payload["creator_id"].(string)
		issueID, _ := payload["issue_id"].(string)
		issueTitle, _ := payload["issue_title"].(string)
		issueStatus, _ := payload["issue_status"].(string)

		if creatorType == "" || creatorID == "" {
			return
		}

		details, _ := json.Marshal(map[string]string{
			"emoji": reaction.Emoji,
		})

		notifyDirect(ctx, queries, bus,
			creatorType, creatorID,
			e.WorkspaceID, e, issueID, issueStatus,
			"reaction_added", "info",
			issueTitle, "",
			details,
		)
	})

	// reaction:added — notify the comment author
	bus.Subscribe(protocol.EventReactionAdded, func(e events.Event) {
		payload, ok := e.Payload.(map[string]any)
		if !ok {
			return
		}

		reaction, ok := payload["reaction"].(handler.ReactionResponse)
		if !ok {
			return
		}

		commentAuthorType, _ := payload["comment_author_type"].(string)
		commentAuthorID, _ := payload["comment_author_id"].(string)
		commentID, _ := payload["comment_id"].(string)
		issueID, _ := payload["issue_id"].(string)
		issueTitle, _ := payload["issue_title"].(string)
		issueStatus, _ := payload["issue_status"].(string)

		if commentAuthorType == "" || commentAuthorID == "" {
			return
		}

		detailsMap := map[string]string{
			"emoji": reaction.Emoji,
		}
		if commentID != "" {
			detailsMap["comment_id"] = commentID
		}
		details, _ := json.Marshal(detailsMap)

		notifyDirect(ctx, queries, bus,
			commentAuthorType, commentAuthorID,
			e.WorkspaceID, e, issueID, issueStatus,
			"reaction_added", "info",
			issueTitle, "",
			details,
		)
	})

	// task:completed — no inbox notification (completion is visible from status change)

	// task:failed — notify all subscribers except the agent
	bus.Subscribe(protocol.EventTaskFailed, func(e events.Event) {
		payload, ok := e.Payload.(map[string]any)
		if !ok {
			return
		}
		agentID, _ := payload["agent_id"].(string)
		issueID, _ := payload["issue_id"].(string)
		if issueID == "" {
			return
		}

		issue, err := queries.GetIssue(ctx, parseUUID(issueID))
		if err != nil {
			slog.Error("task:failed notification: failed to get issue", "issue_id", issueID, "error", err)
			return
		}

		exclude := map[string]bool{}
		if agentID != "" {
			exclude[agentID] = true
		}

		notifySubscribers(ctx, queries, bus, issueID, issue.Status, e.WorkspaceID,
			events.Event{
				Type:        e.Type,
				WorkspaceID: e.WorkspaceID,
				ActorType:   "agent",
				ActorID:     agentID,
			},
			exclude, "task_failed", "action_required",
			issue.Title, "",
			emptyDetails)
	})
}

// inboxItemToResponse converts a db.InboxItem into a map suitable for
// JSON-serializable event payloads (mirrors handler.inboxToResponse fields).
func inboxItemToResponse(item db.InboxItem) map[string]any {
	return map[string]any{
		"id":             util.UUIDToString(item.ID),
		"workspace_id":   util.UUIDToString(item.WorkspaceID),
		"recipient_type": item.RecipientType,
		"recipient_id":   util.UUIDToString(item.RecipientID),
		"type":           item.Type,
		"severity":       item.Severity,
		"issue_id":       util.UUIDToPtr(item.IssueID),
		"title":          item.Title,
		"body":           util.TextToPtr(item.Body),
		"read":           item.Read,
		"archived":       item.Archived,
		"created_at":     util.TimestampToString(item.CreatedAt),
		"actor_type":     util.TextToPtr(item.ActorType),
		"actor_id":       util.UUIDToPtr(item.ActorID),
		"details":        json.RawMessage(item.Details),
	}
}
