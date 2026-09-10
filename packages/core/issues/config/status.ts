import type { IssueStatusCategory } from "../../types";

// These three are keyed on CATEGORY, not on status key. A workspace can define
// any number of custom statuses, but every one of them belongs to exactly one
// of the 7 categories below — so board columns, the presentation config and the
// paginated fetch all keep a fixed shape. Resolve a status KEY to its category
// with the workspace catalog (`useIssueStatuses`) before indexing these.
// (MUL-6243)

export const STATUS_ORDER: IssueStatusCategory[] = [
  "backlog",
  "todo",
  "in_progress",
  "in_review",
  "done",
  "blocked",
  "cancelled",
];

export const ALL_STATUSES: IssueStatusCategory[] = [
  "backlog",
  "todo",
  "in_progress",
  "in_review",
  "done",
  "blocked",
  "cancelled",
];

// Board and swimlane columns use one neutral surface in both themes. Status
// colors belong to their icons and labels, not to yellow/green/blue panels.
export const STATUS_CONFIG: Record<
  IssueStatusCategory,
  {
    label: string;
    iconColor: string;
    hoverBg: string;
    dividerColor: string;
    columnBg: string;
  }
> = {
  backlog: { label: "Backlog", iconColor: "text-gray-500", hoverBg: "hover:bg-gray-500/10", dividerColor: "bg-gray-500", columnBg: "bg-muted/40" },
  todo: { label: "Todo", iconColor: "text-sky-500", hoverBg: "hover:bg-sky-500/10", dividerColor: "bg-sky-500", columnBg: "bg-muted/40" },
  in_progress: { label: "In Progress", iconColor: "text-amber-500", hoverBg: "hover:bg-amber-500/10", dividerColor: "bg-amber-500", columnBg: "bg-muted/40" },
  in_review: { label: "In Review", iconColor: "text-violet-500", hoverBg: "hover:bg-violet-500/10", dividerColor: "bg-violet-500", columnBg: "bg-muted/40" },
  done: { label: "Done", iconColor: "text-green-500", hoverBg: "hover:bg-green-500/10", dividerColor: "bg-green-500", columnBg: "bg-muted/40" },
  blocked: { label: "Blocked", iconColor: "text-red-500", hoverBg: "hover:bg-red-500/10", dividerColor: "bg-red-500", columnBg: "bg-muted/40" },
  cancelled: { label: "Cancelled", iconColor: "text-orange-500", hoverBg: "hover:bg-orange-500/10", dividerColor: "bg-orange-500", columnBg: "bg-muted/40" },
};
