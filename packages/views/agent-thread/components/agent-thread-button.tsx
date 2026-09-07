"use client";

import { useCallback, useEffect, useRef } from "react";
import { MessagesSquare } from "lucide-react";
import { useAgentThreadPanelStore } from "@patchbay/core/agent-thread";
import type { AgentTask } from "@patchbay/core/types";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { cn } from "@patchbay/ui/lib/utils";
import { Tooltip, TooltipContent, TooltipTrigger } from "@patchbay/ui/components/ui/tooltip";
import { useNavigation } from "../../navigation";

export function AgentThreadButton({
  task,
  title,
  className,
  renderButton = true,
  open: controlledOpen,
  onOpenChange,
}: {
  task: AgentTask;
  title: string;
  className?: string;
  renderButton?: boolean;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}) {
  const routeWorkspaceId = useWorkspaceId();
  const workspaceId = task.workspace_id || routeWorkspaceId;
  const { pathname } = useNavigation();
  const panel = useAgentThreadPanelStore((state) => state.panel);
  const openPanel = useAgentThreadPanelStore((state) => state.openPanel);
  const closePanel = useAgentThreadPanelStore((state) => state.closePanel);
  const isPanelOpen = panel?.workspaceId === workspaceId && panel.taskId === task.id;
  const observedControlledOpen = useRef(false);
  const open = useCallback(() => {
    openPanel({ workspaceId, taskId: task.id, routePath: pathname });
    onOpenChange?.(true);
  }, [onOpenChange, openPanel, pathname, task.id, workspaceId]);

  useEffect(() => {
    if (controlledOpen === true && !isPanelOpen && !observedControlledOpen.current) {
      openPanel({ workspaceId, taskId: task.id, routePath: pathname });
    } else if (controlledOpen === false && isPanelOpen) {
      closePanel();
    }
  }, [closePanel, controlledOpen, isPanelOpen, openPanel, pathname, task.id, workspaceId]);

  useEffect(() => {
    if (controlledOpen !== true) {
      observedControlledOpen.current = false;
      return;
    }
    if (observedControlledOpen.current && !isPanelOpen) {
      onOpenChange?.(false);
    }
    observedControlledOpen.current = isPanelOpen;
  }, [controlledOpen, isPanelOpen, onOpenChange]);

  return (
    renderButton ? (
      <Tooltip>
        <TooltipTrigger render={<button type="button" onClick={open} aria-label={title} className={cn("flex items-center justify-center rounded p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground", className)} />}>
          <MessagesSquare className="size-3.5" />
        </TooltipTrigger>
        <TooltipContent>{title}</TooltipContent>
      </Tooltip>
    ) : null
  );
}
