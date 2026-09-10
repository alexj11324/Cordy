import { Circle, CircleCheck, CircleDot, CirclePause, CircleX, Clock, type LucideIcon } from "lucide-react";
import { isIssueStatusCategory } from "@orvilo/core/issue-statuses";
import { statusCategoryOfKey } from "@orvilo/core/issues";
import type { IssueStatus, IssueStatusCategory } from "@orvilo/core/types";
import { STATUS_CONFIG } from "@orvilo/core/issues/config";

// ReUI kanban-board-2's built-in icon family, shared by every task surface.
const STATUS_ICONS: Record<IssueStatusCategory, LucideIcon> = {
  backlog: CircleDot,
  todo: Circle,
  in_progress: CircleDot,
  in_review: Clock,
  done: CircleCheck,
  blocked: CircleX,
  cancelled: CirclePause,
};

export function StatusIcon({
  status,
  category: categoryProp,
  color,
  className = "h-4 w-4",
  inheritColor = false,
}: {
  status: IssueStatus | string;
  /** Custom keys use their resolved workspace category; unknown keys fall back to todo. */
  category?: IssueStatusCategory;
  /** Custom colors override the category token unless the caller inherits its color. */
  color?: string | null;
  className?: string;
  inheritColor?: boolean;
}) {
  const category = categoryProp ?? statusCategoryOfKey(status);
  const cfg = STATUS_CONFIG[category];
  const knownCategory = categoryProp !== undefined || isIssueStatusCategory(status);
  const Icon = STATUS_ICONS[category] ?? Circle;
  const useCustomColor = !inheritColor && Boolean(color);
  return (
    <Icon
      data-slot="issue-status-icon"
      aria-hidden="true"
      style={useCustomColor ? { color: color ?? undefined } : undefined}
      className={`${className} ${inheritColor || useCustomColor ? "" : knownCategory ? cfg?.iconColor ?? "text-muted-foreground" : "text-muted-foreground"} shrink-0`}
    />
  );
}
