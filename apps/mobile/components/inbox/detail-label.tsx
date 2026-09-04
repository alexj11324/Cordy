/**
 * Mobile InboxDetailLabel — type-aware second-line for inbox rows.
 *
 * Mirrors the per-type structure of
 * packages/views/inbox/components/inbox-detail-label.tsx. Role details use
 * explicit owner/executor wording so an incomplete executor payload cannot
 * be rendered as a member assignment.
 *
 * Web is i18n-driven (useT). Mobile v1 is English-only; when mobile ships
 * i18n, mirror the namespace structure.
 */
import { View } from "react-native";
import type {
  InboxItem,
  InboxItemType,
  IssuePriority,
} from "@patchbay/core/types";
import { formatDateOnly } from "@patchbay/core/issues/date";
import { Text } from "@/components/ui/text";
import { StatusIcon } from "@/components/ui/status-icon";
import { PriorityIcon } from "@/components/ui/priority-icon";
import { useActorLookup } from "@/data/use-actor-name";
import { useIssueStatuses } from "@/lib/use-issue-statuses";
import { cn } from "@/lib/utils";
import {
  resolveInboxRoleActorType,
  type InboxIssueRole,
} from "@/lib/inbox-role";

// Mirrors PRIORITY_CONFIG.label in packages/core/issues/config/priority.ts
const PRIORITY_LABEL: Record<IssuePriority, string> = {
  urgent: "Urgent",
  high: "High",
  medium: "Medium",
  low: "Low",
  none: "No priority",
};

// Mirrors useTypeLabels in packages/views/inbox/components/inbox-detail-label.tsx
const TYPE_LABEL: Record<InboxItemType, string> = {
  issue_assigned: "Role assigned",
  issue_subscribed: "Subscribed",
  unassigned: "Role cleared",
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

// due_date is a calendar day — format timezone-safely (no offset day shift).
function shortDate(dateStr: string): string {
  return formatDateOnly(dateStr, { month: "short", day: "numeric" }, "en-US");
}

function singleLine(value: string | null | undefined): string {
  return (value ?? "").replace(/\s+/g, " ").trim();
}

function roleActorName(
  role: InboxIssueRole,
  rawType: string | undefined,
  id: string,
  getName: (
    type: "member" | "agent" | "team" | null | undefined,
    id: string | null | undefined,
  ) => string,
): string | null {
  const actorType = resolveInboxRoleActorType(role, rawType);
  return actorType ? getName(actorType, id) : null;
}

export function InboxDetailLabel({
  item,
  className,
}: {
  item: InboxItem;
  className?: string;
}) {
  const { getName } = useActorLookup();
  // `details.to` is a status KEY and may be a custom one, so its name, colour
  // and glyph all resolve through the workspace catalog. (MUL-6243)
  const { categoryOf, colorOf, labelOf } = useIssueStatuses();
  const details = item.details ?? {};

  // Cases with inline icons → Row layout.
  if (item.type === "status_changed" && details.to) {
    const status = details.to;
    return (
      <View className={cn("flex-row items-center gap-1", className)}>
        <Text className="text-xs text-muted-foreground">Set status to</Text>
        <StatusIcon
          status={status}
          category={categoryOf(status)}
          color={colorOf(status)}
          size={12}
        />
        <Text className="text-xs text-muted-foreground" numberOfLines={1}>
          {labelOf(status)}
        </Text>
      </View>
    );
  }

  if (item.type === "priority_changed" && details.to) {
    const priority = details.to as IssuePriority;
    return (
      <View className={cn("flex-row items-center gap-1", className)}>
        <Text className="text-xs text-muted-foreground">Set priority to</Text>
        <PriorityIcon priority={priority} size={12} />
        <Text className="text-xs text-muted-foreground" numberOfLines={1}>
          {PRIORITY_LABEL[priority] ?? priority}
        </Text>
      </View>
    );
  }

  // Single-string cases.
  const text = (() => {
    switch (item.type) {
      case "issue_assigned":
        if (details.new_owner_id) {
          const name = roleActorName(
            "owner",
            details.new_owner_type,
            details.new_owner_id,
            getName,
          );
          return name ? `Set owner to ${name}` : "Owner assigned";
        }
        if (details.new_executor_id) {
          const name = roleActorName(
            "executor",
            details.new_executor_type,
            details.new_executor_id,
            getName,
          );
          return name ? `Set executor to ${name}` : "Executor assigned";
        }
        return TYPE_LABEL[item.type];
      case "unassigned":
        if (details.prev_owner_id) return "Removed owner";
        if (details.prev_executor_id) return "Removed executor";
        return TYPE_LABEL[item.type];
      case "owner_changed":
        if (details.new_owner_id) {
          const name = roleActorName(
            "owner",
            details.new_owner_type,
            details.new_owner_id,
            getName,
          );
          return name ? `Set owner to ${name}` : TYPE_LABEL[item.type];
        }
        return TYPE_LABEL[item.type];
      case "executor_changed":
        if (details.new_executor_id) {
          const name = roleActorName(
            "executor",
            details.new_executor_type,
            details.new_executor_id,
            getName,
          );
          return name ? `Set executor to ${name}` : TYPE_LABEL[item.type];
        }
        return TYPE_LABEL[item.type];
      case "due_date_changed":
        return details.to
          ? `Set due date to ${shortDate(details.to)}`
          : "Removed due date";
      case "new_comment":
        return singleLine(item.body) || TYPE_LABEL[item.type];
      case "reaction_added":
        return details.emoji
          ? `Reacted with ${details.emoji}`
          : TYPE_LABEL[item.type];
      case "quick_create_done":
        return details.identifier
          ? `Created with agent: ${details.identifier}`
          : TYPE_LABEL[item.type];
      case "quick_create_failed": {
        const detail = singleLine(details.error) || singleLine(item.body);
        return detail ? `Failed: ${detail}` : TYPE_LABEL[item.type];
      }
      // Mirrors packages/views/inbox/components/inbox-detail-label.tsx: the
      // unconfirmed outcome deliberately drops the "Failed:" prefix, because
      // the issue may actually have been created.
      case "quick_create_unconfirmed": {
        const detail = singleLine(details.error) || singleLine(item.body);
        return detail || TYPE_LABEL[item.type];
      }
      default:
        return TYPE_LABEL[item.type] ?? item.type;
    }
  })();

  return (
    <Text
      className={cn("text-xs text-muted-foreground", className)}
      numberOfLines={1}
    >
      {text}
    </Text>
  );
}
