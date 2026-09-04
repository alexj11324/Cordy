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
import type { InboxItem, IssuePriority } from "@patchbay/core/types";
import { Text } from "@/components/ui/text";
import { StatusIcon } from "@/components/ui/status-icon";
import { PriorityIcon } from "@/components/ui/priority-icon";
import { useActorLookup } from "@/data/use-actor-name";
import { inboxDetailText } from "@/lib/inbox-detail-text";
import { useIssueStatuses } from "@/lib/use-issue-statuses";
import { cn } from "@/lib/utils";

// Mirrors PRIORITY_CONFIG.label in packages/core/issues/config/priority.ts
const PRIORITY_LABEL: Record<IssuePriority, string> = {
  urgent: "Urgent",
  high: "High",
  medium: "Medium",
  low: "Low",
  none: "No priority",
};

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

  const text = inboxDetailText(item, (type, id) => {
    if (type === "member" || type === "agent" || type === "team") {
      return getName(type, id);
    }
    return "Unknown";
  });

  return (
    <Text
      className={cn("text-xs text-muted-foreground", className)}
      numberOfLines={1}
    >
      {text}
    </Text>
  );
}
