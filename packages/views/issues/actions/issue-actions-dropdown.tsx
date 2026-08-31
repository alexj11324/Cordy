"use client";

import { useState, type ReactElement } from "react";
import type { Issue } from "@patchbay/core/types";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
} from "@patchbay/ui/components/ui/dropdown-menu";
import { useIssueActions } from "./use-issue-actions";
import {
  IssueActionsMenuItems,
  dropdownPrimitives,
} from "./issue-actions-menu-items";
import { ExecutorPicker } from "../components/pickers";

interface IssueActionsDropdownProps {
  issue: Issue;
  /** A single React element cloned by Base UI as the trigger (via `render` prop). */
  trigger: ReactElement;
  align?: "start" | "end" | "center";
  /** If set, leave the page after the issue is deleted — back to wherever the
   *  user came from, or to this path when there is no in-app history. */
  onDeletedFallbackPath?: string;
}

export function IssueActionsDropdown({
  issue,
  trigger,
  align = "end",
  onDeletedFallbackPath,
}: IssueActionsDropdownProps) {
  const actions = useIssueActions(issue);
  const [executorOpen, setExecutorOpen] = useState(false);

  // The outer `relative inline-flex` is the picker's anchor box: the
  // absolute, pointer-events-none span inside `triggerRender` fills it, so
  // the popover positions itself relative to the dropdown's 3-dot button
  // without us having to thread a ref through Base UI's anchor API.
  return (
    <span className="relative inline-flex">
      <DropdownMenu>
        <DropdownMenuTrigger render={trigger} />
        <DropdownMenuContent align={align} className="w-auto">
          <IssueActionsMenuItems
            issue={issue}
            actions={actions}
            primitives={dropdownPrimitives}
            onOpenExecutor={() => setExecutorOpen(true)}
            onDeletedFallbackPath={onDeletedFallbackPath}
          />
        </DropdownMenuContent>
      </DropdownMenu>
      {/* Mount the picker only once the user actually opens it. Otherwise
          every row in a list/board would subscribe to members/agents/teams
          /frequency queries on mount, multiplying memory + render cost. */}
      {executorOpen && (
        <ExecutorPicker
          executorType={issue.executor_type}
          executorId={issue.executor_id}
          onUpdate={actions.updateField}
          open={executorOpen}
          onOpenChange={setExecutorOpen}
          triggerRender={
            <span
              aria-hidden
              className="pointer-events-none absolute inset-0"
            />
          }
          trigger={<span />}
          align={align}
        />
      )}
    </span>
  );
}
