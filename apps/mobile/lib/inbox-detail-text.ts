import type { InboxItem, InboxItemType } from "@patchbay/core/types";
import { formatDateOnly } from "@patchbay/core/issues/date";

const TYPE_LABEL: Record<InboxItemType, string> = {
  issue_assigned: "Assigned",
  issue_subscribed: "Subscribed",
  unassigned: "Unassigned",
  owner_changed: "Owner changed",
  executor_changed: "Executor changed",
  status_changed: "Status changed",
  priority_changed: "Priority changed",
  start_date_changed: "Start date changed",
  due_date_changed: "Due date changed",
  new_comment: "New comment",
  mentioned: "Mentioned",
  review_requested: "Review requested",
  task_completed: "Task completed",
  task_failed: "Task failed",
  agent_blocked: "Agent blocked",
  agent_completed: "Agent completed",
  reaction_added: "Reaction added",
  quick_create_done: "Quick-create done",
  quick_create_failed: "Quick-create failed",
  quick_create_unconfirmed: "Quick-create needs a check",
};

function typeLabel(type: string): string {
  return Object.prototype.hasOwnProperty.call(TYPE_LABEL, type)
    ? TYPE_LABEL[type as InboxItemType]
    : type;
}

function singleLine(value: string | null | undefined): string {
  return (value ?? "").replace(/\s+/g, " ").trim();
}

export function inboxDetailText(
  item: InboxItem,
  resolveActorName: (type: string | null | undefined, id: string) => string,
): string {
  const details = item.details ?? {};
  switch (item.type) {
    case "issue_assigned":
      if (details.new_owner_id) {
        return `Set owner to ${resolveActorName(details.new_owner_type, details.new_owner_id)}`;
      }
      if (details.new_executor_id) {
        return `Set executor to ${resolveActorName(details.new_executor_type, details.new_executor_id)}`;
      }
      return typeLabel(item.type);
    case "unassigned":
      if (details.prev_owner_id) return "Removed owner";
      if (details.prev_executor_id) return "Removed executor";
      return typeLabel(item.type);
    case "owner_changed":
      return details.new_owner_id
        ? `Set owner to ${resolveActorName(details.new_owner_type, details.new_owner_id)}`
        : typeLabel(item.type);
    case "executor_changed":
      return details.new_executor_id
        ? `Set executor to ${resolveActorName(details.new_executor_type, details.new_executor_id)}`
        : typeLabel(item.type);
    case "review_requested":
      return details.new_reviewer_id
        ? `Review requested for ${resolveActorName(details.new_reviewer_type, details.new_reviewer_id)}`
        : typeLabel(item.type);
    case "due_date_changed":
      return details.to
        ? `Set due date to ${formatDateOnly(details.to, { month: "short", day: "numeric" }, "en-US")}`
        : "Removed due date";
    case "new_comment":
      return singleLine(item.body) || typeLabel(item.type);
    case "reaction_added":
      return details.emoji
        ? `Reacted with ${details.emoji}`
        : typeLabel(item.type);
    case "quick_create_done":
      return details.identifier
        ? `Created with agent: ${details.identifier}`
        : typeLabel(item.type);
    case "quick_create_failed": {
      const detail = singleLine(details.error) || singleLine(item.body);
      return detail ? `Failed: ${detail}` : typeLabel(item.type);
    }
    case "quick_create_unconfirmed": {
      const detail = singleLine(details.error) || singleLine(item.body);
      return detail || typeLabel(item.type);
    }
    default:
      return typeLabel(item.type);
  }
}
