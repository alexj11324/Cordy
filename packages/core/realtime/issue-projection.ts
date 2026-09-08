"use client";

import { onInboxIssueDeleted, onInboxIssueStatusChanged } from "../inbox/ws-updaters";
import {
  invalidateLastActivitySortedIssueLists,
  invalidateUpdatedAtSortedIssueLists,
} from "../issues/cache-coordinator";
import { issueKeys } from "../issues/queries";
import {
  invalidateIssueOwnerProjections,
  onIssueAuxiliaryRevision,
  onIssueCreated,
  onIssueDeleted,
  onIssueLabelsChanged,
  onIssueMetadataChanged,
  onIssuePropertiesChanged,
  onIssueUpdated,
} from "../issues/ws-updaters";
import { propertyKeys } from "../properties/queries";
import type {
  ActivityCreatedPayload,
  CommentCreatedPayload,
  CommentDeletedPayload,
  CommentResolvedPayload,
  CommentUnresolvedPayload,
  CommentUpdatedPayload,
  IssueAttachmentsChangedPayload,
  IssueCreatedPayload,
  IssueDeletedPayload,
  IssueLabelsChangedPayload,
  IssueMetadataChangedPayload,
  IssuePropertiesChangedPayload,
  IssueReactionAddedPayload,
  IssueReactionRemovedPayload,
  IssueUpdatedPayload,
  ReactionAddedPayload,
  ReactionRemovedPayload,
  SubscriberAddedPayload,
  SubscriberRemovedPayload
} from "../types";

import type { ProjectionContext } from "./projection-context";
export function subscribeIssueProjection({ ws, qc, workspaceId }: ProjectionContext) {
  // --- Specific event handlers (granular cache updates) ---
  // No self-event filtering: actor_id identifies the USER, not the TAB.
  // Filtering by actor_id would block other tabs of the same user.
  // Instead, both mutations and WS handlers use dedup checks to be idempotent.

  const unsubIssueUpdated = ws.on("issue:updated", (p) => {
    const payload = p as IssueUpdatedPayload;
    const { issue } = payload;
    if (!issue?.id) return;
    const wsId = workspaceId;
    if (wsId) {
      onIssueUpdated(qc, wsId, issue, {
        ownerChanged: payload.owner_changed,
        executorChanged: payload.executor_changed,
        statusChanged: payload.status_changed,
        projectChanged: payload.project_changed,
      });
      if (issue.status) {
        onInboxIssueStatusChanged(qc, wsId, issue.id, issue.status);
      }
    }
  });

  const unsubIssueCreated = ws.on("issue:created", (p) => {
    const { issue } = p as IssueCreatedPayload;
    if (!issue) return;
    const wsId = workspaceId;
    if (wsId) onIssueCreated(qc, wsId, issue);
  });

  const unsubIssueDeleted = ws.on("issue:deleted", (p) => {
    const { issue_id } = p as IssueDeletedPayload;
    if (!issue_id) return;
    const wsId = workspaceId;
    if (wsId) {
      onIssueDeleted(qc, wsId, issue_id);
      onInboxIssueDeleted(qc, wsId, issue_id);
    }
  });

  const unsubIssueLabelsChanged = ws.on("issue_labels:changed", (p) => {
    const { issue_id, labels, issue_revision } = p as IssueLabelsChangedPayload;
    if (!issue_id) return;
    const wsId = workspaceId;
    if (wsId) onIssueLabelsChanged(qc, wsId, issue_id, labels ?? [], issue_revision);
  });

  const unsubIssueAttachmentsChanged = ws.on("issue_attachments:changed", (p) => {
    const { issue_id, issue_revision } = p as IssueAttachmentsChangedPayload;
    if (!issue_id) return;
    qc.invalidateQueries({ queryKey: issueKeys.attachments(issue_id) });
    const wsId = workspaceId;
    if (wsId) onIssueAuxiliaryRevision(qc, wsId, issue_id, issue_revision);
  });

  const unsubIssueMetadataChanged = ws.on("issue_metadata:changed", (p) => {
    const { issue_id, metadata, issue_revision } = p as IssueMetadataChangedPayload;
    if (!issue_id) return;
    const wsId = workspaceId;
    if (wsId) onIssueMetadataChanged(qc, wsId, issue_id, metadata ?? {}, issue_revision);
  });

  const unsubIssuePropertiesChanged = ws.on("issue_properties:changed", (p) => {
    const { issue_id, properties, issue_revision } = p as IssuePropertiesChangedPayload;
    if (!issue_id) return;
    const wsId = workspaceId;
    if (wsId) {
      onIssuePropertiesChanged(qc, wsId, issue_id, properties ?? {}, issue_revision);
      // The catalog embeds per-definition usage counts; every value
      // set/unset shifts them. The list is tiny, so a refetch beats
      // trying to patch counts client-side.
      qc.invalidateQueries({ queryKey: propertyKeys.all(wsId) });
    }
  });

  // Definition changes (create / rename / options / archive) — refetch the
  // catalog; issue caches keep raw value bags so they stay valid.
  const unsubPropertyChanged = ["property:created", "property:updated"].map((event) =>
    ws.on(event as "property:created" | "property:updated", () => {
      const wsId = workspaceId;
      if (wsId) {
        qc.invalidateQueries({ queryKey: propertyKeys.all(wsId) });
        // Group order, supported group types, and unavailable option values
        // are derived from the property definition, not just issue rows.
        qc.invalidateQueries({ queryKey: issueKeys.tableAll(wsId) });
      }
    }),
  );

  // --- Timeline event handlers (global fallback) ---
  // These events are also handled granularly by useIssueTimeline when
  // IssueDetail is mounted. This global handler exists to mark the
  // timeline cache stale for issues whose IssueDetail is *not* mounted,
  // so stale data isn't served on next mount (staleTime: Infinity, set on
  // the QueryClient default, relies on this).
  //
  // `refetchType: "none"` is the load-bearing detail: without it, an
  // active IssueDetail observer would refetch the entire timeline on
  // every comment / activity / reaction event. The refetch replaces
  // every entry's reference and busts React.memo on every CommentCard
  // subtree (visible during AI streaming as a flash across all sibling
  // threads, MUL-1941). Inactive observers don't refetch either way;
  // when IssueDetail mounts later, the stale flag triggers the refetch
  // through `refetchOnMount`. Active observers stay fresh via the
  // granular setQueryData handlers in `useIssueTimeline`.
  const invalidateTimeline = (issueId: string) => {
    qc.invalidateQueries({
      queryKey: issueKeys.timeline(issueId),
      refetchType: "none",
    });
  };

  const unsubCommentCreated = ws.on("comment:created", (p) => {
    const { comment, issue_revision: issueRevision } = p as CommentCreatedPayload;
    if (!comment?.issue_id) return;
    invalidateTimeline(comment.issue_id);
    // A new comment bumps the parent issue's updated_at server-side
    // (MUL-5009), so any open board/list sorted by "Updated date" has
    // drifted. Refetch just those keys to re-sort the commented card into
    // place; every other sort is untouched. Only comment:created bumps
    // updated_at, so the other comment events below deliberately do not.
    const wsId = workspaceId;
    if (wsId) {
      invalidateUpdatedAtSortedIssueLists(qc, wsId);
      invalidateLastActivitySortedIssueLists(qc, wsId);
      // A comment carries only the aggregate owner revision, not a full
      // Issue snapshot. Mark stale projections for an authoritative fetch
      // without making a later full snapshot look older than the cache.
      if (issueRevision) {
        onIssueAuxiliaryRevision(qc, wsId, comment.issue_id, issueRevision);
      } else {
        invalidateIssueOwnerProjections(qc, wsId, comment.issue_id);
      }
    }
  });

  const unsubCommentUpdated = ws.on("comment:updated", (p) => {
    const { comment, issue_revision } = p as CommentUpdatedPayload;
    if (!comment?.issue_id) return;
    invalidateTimeline(comment.issue_id);
    const wsId = workspaceId;
    if (wsId) {
      invalidateLastActivitySortedIssueLists(qc, wsId);
      if (issue_revision) {
        onIssueAuxiliaryRevision(qc, wsId, comment.issue_id, issue_revision);
      } else {
        invalidateIssueOwnerProjections(qc, wsId, comment.issue_id);
      }
    }
  });

  const unsubCommentDeleted = ws.on("comment:deleted", (p) => {
    const { issue_id, issue_revision } = p as CommentDeletedPayload;
    if (!issue_id) return;
    invalidateTimeline(issue_id);
    const wsId = workspaceId;
    if (wsId) {
      invalidateLastActivitySortedIssueLists(qc, wsId);
      if (issue_revision) {
        onIssueAuxiliaryRevision(qc, wsId, issue_id, issue_revision);
      } else {
        invalidateIssueOwnerProjections(qc, wsId, issue_id);
      }
    }
  });

  const unsubCommentResolved = ws.on("comment:resolved", (p) => {
    const { comment } = p as CommentResolvedPayload;
    if (comment?.issue_id) invalidateTimeline(comment.issue_id);
  });

  const unsubCommentUnresolved = ws.on("comment:unresolved", (p) => {
    const { comment } = p as CommentUnresolvedPayload;
    if (comment?.issue_id) invalidateTimeline(comment.issue_id);
  });

  const unsubActivityCreated = ws.on("activity:created", (p) => {
    const { issue_id } = p as ActivityCreatedPayload;
    if (issue_id) invalidateTimeline(issue_id);
  });

  const unsubReactionAdded = ws.on("reaction:added", (p) => {
    const { issue_id } = p as ReactionAddedPayload;
    if (issue_id) invalidateTimeline(issue_id);
  });

  const unsubReactionRemoved = ws.on("reaction:removed", (p) => {
    const { issue_id } = p as ReactionRemovedPayload;
    if (issue_id) invalidateTimeline(issue_id);
  });

  // --- Issue-level reactions & subscribers (global fallback) ---

  const unsubIssueReactionAdded = ws.on("issue_reaction:added", (p) => {
    const { issue_id, issue_revision } = p as IssueReactionAddedPayload;
    if (issue_id) {
      qc.invalidateQueries({ queryKey: issueKeys.reactions(issue_id) });
      const wsId = workspaceId;
      if (wsId) onIssueAuxiliaryRevision(qc, wsId, issue_id, issue_revision);
    }
  });

  const unsubIssueReactionRemoved = ws.on("issue_reaction:removed", (p) => {
    const { issue_id, issue_revision } = p as IssueReactionRemovedPayload;
    if (issue_id) {
      qc.invalidateQueries({ queryKey: issueKeys.reactions(issue_id) });
      const wsId = workspaceId;
      if (wsId) onIssueAuxiliaryRevision(qc, wsId, issue_id, issue_revision);
    }
  });

  const unsubSubscriberAdded = ws.on("subscriber:added", (p) => {
    const { issue_id } = p as SubscriberAddedPayload;
    if (issue_id) qc.invalidateQueries({ queryKey: issueKeys.subscribers(issue_id) });
  });

  const unsubSubscriberRemoved = ws.on("subscriber:removed", (p) => {
    const { issue_id } = p as SubscriberRemovedPayload;
    if (issue_id) qc.invalidateQueries({ queryKey: issueKeys.subscribers(issue_id) });
  });

  return () => {
    unsubIssueUpdated();
    unsubIssueCreated();
    unsubIssueDeleted();
    unsubIssueAttachmentsChanged();
    unsubIssueLabelsChanged();
    unsubIssueMetadataChanged();
    unsubIssuePropertiesChanged();
    unsubPropertyChanged.forEach((unsub) => unsub());
    unsubCommentCreated();
    unsubCommentUpdated();
    unsubCommentDeleted();
    unsubCommentResolved();
    unsubCommentUnresolved();
    unsubActivityCreated();
    unsubReactionAdded();
    unsubReactionRemoved();
    unsubIssueReactionAdded();
    unsubIssueReactionRemoved();
    unsubSubscriberAdded();
    unsubSubscriberRemoved();
  };
}
