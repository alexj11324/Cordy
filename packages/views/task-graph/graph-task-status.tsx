import type { DependencyGraphNode } from "@orvilo/core/types";
import { isIssueStatusCategory } from "@orvilo/core/issue-statuses";
import { StatusIcon } from "../issues/components/status-icon";
import { cn } from "@orvilo/ui/lib/utils";

export function GraphTaskStatus({
  node,
  label,
  className,
}: {
  node: DependencyGraphNode;
  label: string;
  className?: string;
}) {
  const status = node.issue?.status ?? node.status ?? "todo";
  const category = node.issue?.status_category ?? node.status_category;
  const resolved = isIssueStatusCategory(category)
    ? category
    : isIssueStatusCategory(status)
      ? status
      : "todo";
  return (
    <span
      className={cn(
        "inline-flex items-center gap-2 text-sm text-foreground",
        className,
      )}
      data-graph-status={resolved}
    >
      <StatusIcon status={status} category={resolved} className="size-3.5" />
      <span>{label}</span>
    </span>
  );
}
