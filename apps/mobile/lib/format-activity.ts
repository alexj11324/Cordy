/**
 * Activity-row text formatter. Subset of the web `formatActivity` in
 * packages/views/issues/components/issue-detail.tsx:95 — same actions. Review
 * assignment/handoff copy follows the account language; legacy non-review
 * actions retain their existing English copy until their own migration.
 *
 * Unknown actions fall through to the raw string in `entry.action`. NEVER
 * throw and NEVER drop the row — that's the API Response Compatibility rule
 * from repo-root CLAUDE.md (server may add new action enum values; older
 * mobile clients in the wild must render them as a generic fallback, not
 * crash).
 */
import type {
  IssuePriority,
  IssueStatusCategory,
  TimelineEntry,
} from "@patchbay/core/types";
import { formatDateOnly } from "@patchbay/core/issues/date";
import { STATUS_LABEL, isIssueStatusCategory } from "@/lib/issue-status";
import { formatIssueRoleCopy, getIssueRoleCopy } from "@/lib/issue-role-copy";

const PRIORITY_LABEL: Record<IssuePriority, string> = {
  urgent: "Urgent",
  high: "High",
  medium: "Medium",
  low: "Low",
  none: "No priority",
};

/**
 * Names a status KEY out of a timeline entry. `resolveLabel` comes from the
 * workspace catalog and is what names a CUSTOM status; without it (or for a key
 * the catalog never heard of) a built-in still gets its own copy and anything
 * else falls back to the raw key rather than rendering blank. Mirrors web's
 * `statusLabel` in packages/views/issues/components/issue-detail.tsx.
 * (MUL-6243)
 */
function statusName(
  s: string | undefined,
  resolveLabel?: (statusKey: string) => string,
): string {
  if (!s) return "?";
  if (resolveLabel) return resolveLabel(s);
  return isIssueStatusCategory(s) ? STATUS_LABEL[s] : s;
}

function priorityName(p: string | undefined): string {
  if (p && Object.prototype.hasOwnProperty.call(PRIORITY_LABEL, p)) {
    return PRIORITY_LABEL[p as IssuePriority];
  }
  return p ?? "?";
}

function detailString(
  details: Record<string, unknown>,
  key: string,
): string | undefined {
  const value = details[key];
  return typeof value === "string" ? value : undefined;
}

function reviewActivityIsHandoff(
  fromStatus: string | undefined,
  toStatus: string | undefined,
  resolveCategory?: (statusKey: string) => IssueStatusCategory,
): boolean {
  if (!fromStatus || !toStatus) return false;
  const fromCategory =
    resolveCategory?.(fromStatus) ??
    (isIssueStatusCategory(fromStatus) ? fromStatus : undefined);
  const toCategory =
    resolveCategory?.(toStatus) ??
    (isIssueStatusCategory(toStatus) ? toStatus : undefined);
  if (fromCategory && toCategory) {
    return fromCategory !== "in_review" && toCategory === "in_review";
  }
  return false;
}

// start_date / due_date are calendar days — format timezone-safely (no offset
// day shift). Mirrors web's formatActivity in issue-detail.tsx.
function shortDate(date: string | undefined): string {
  if (!date) return "?";
  return formatDateOnly(date, { month: "short", day: "numeric" }, "en-US");
}

export function formatActivity(
  entry: TimelineEntry,
  resolveActorName: (
    type: string | null | undefined,
    id: string | null | undefined,
  ) => string,
  resolveStatusLabel?: (statusKey: string) => string,
  resolveStatusCategory?: (statusKey: string) => IssueStatusCategory,
  language?: string | null,
): string {
  const details = entry.details ?? {};
  switch (entry.action) {
    case "created":
      return "created the issue";
    case "status_changed":
      return `changed status: ${statusName(detailString(details, "from"), resolveStatusLabel)} → ${statusName(detailString(details, "to"), resolveStatusLabel)}`;
    case "priority_changed":
      return `changed priority: ${priorityName(detailString(details, "from"))} → ${priorityName(detailString(details, "to"))}`;
    case "executor_changed": {
      const toType = detailString(details, "to_type");
      const toId = detailString(details, "to_id");
      const fromId = detailString(details, "from_id");
      const isSelf = toType === entry.actor_type && toId === entry.actor_id;
      if (isSelf) return "set themselves as executor";
      if (fromId && !toId) return "removed executor";
      const toName = toId && toType ? resolveActorName(toType, toId) : null;
      if (toName) return `set executor to ${toName}`;
      return "changed executor";
    }
    case "owner_changed": {
      const toType = detailString(details, "to_type");
      const toId = detailString(details, "to_id");
      const fromId = detailString(details, "from_id");
      const isSelf = toType === entry.actor_type && toId === entry.actor_id;
      if (isSelf) return "set themselves as owner";
      if (fromId && !toId) return "removed owner";
      const toName = toId && toType ? resolveActorName(toType, toId) : null;
      if (toName) return `set owner to ${toName}`;
      return "changed owner";
    }
    case "review_handoff": {
      const copy = getIssueRoleCopy(language);
      const fromType = detailString(details, "from_type");
      const fromId = detailString(details, "from_id");
      const toType = detailString(details, "to_type");
      const toId = detailString(details, "to_id");
      const fromName =
        fromId && fromType ? resolveActorName(fromType, fromId) : "?";
      const toName = toId && toType ? resolveActorName(toType, toId) : "?";

      // The activity API uses one action for reviewer assignment/replacement
      // and a real handoff. The status pair is the contract-level
      // discriminator: a reviewer-only change leaves the status unchanged;
      // entering review transfers the review from the executor.
      if (
        reviewActivityIsHandoff(
          detailString(details, "from_status"),
          detailString(details, "to_status"),
          resolveStatusCategory,
        )
      ) {
        return formatIssueRoleCopy(copy.reviewHandoffFromTo, {
          from: fromName,
          to: toName,
        });
      }
      if (!toId) return copy.reviewerRemoved;
      if (!fromId) {
        return formatIssueRoleCopy(copy.reviewerAssignedTo, { name: toName });
      }
      return formatIssueRoleCopy(copy.reviewerChangedFromTo, {
        from: fromName,
        to: toName,
      });
    }
    case "start_date_changed": {
      const to = detailString(details, "to");
      if (!to) return "removed start date";
      return `set start date to ${shortDate(to)}`;
    }
    case "due_date_changed": {
      const to = detailString(details, "to");
      if (!to) return "removed due date";
      return `set due date to ${shortDate(to)}`;
    }
    case "title_changed":
      return `renamed: "${detailString(details, "from") ?? "?"}" → "${detailString(details, "to") ?? "?"}"`;
    case "description_updated":
      return "updated description";
    case "task_completed": {
      const n = entry.coalesced_count ?? 1;
      return n > 1 ? `completed ${n} tasks` : "completed a task";
    }
    case "task_failed": {
      const n = entry.coalesced_count ?? 1;
      return n > 1 ? `failed ${n} tasks` : "failed a task";
    }
    case "team_leader_evaluated": {
      // Copy mirrors packages/views/locales/en/issues.json
      // (team_leader_action / team_leader_no_action / team_leader_failed,
      // each with an optional `_reason` variant).
      const reason = detailString(details, "reason")?.trim();
      switch (detailString(details, "outcome")) {
        case "action":
          return reason
            ? `evaluated and took action: ${reason}`
            : "evaluated and took action";
        case "no_action":
          return reason
            ? `evaluated: no action needed (${reason})`
            : "evaluated: no action needed";
        case "failed":
          return reason ? `evaluation failed: ${reason}` : "evaluation failed";
        default:
          return "evaluated the team trigger";
      }
    }
    default:
      return entry.action ?? "";
  }
}
