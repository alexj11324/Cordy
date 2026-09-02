"use client";

import { useState } from "react";
import { MessagesSquare } from "lucide-react";
import type { AgentTask } from "@patchbay/core/types";
import { cn } from "@patchbay/ui/lib/utils";
import { Tooltip, TooltipContent, TooltipTrigger } from "@patchbay/ui/components/ui/tooltip";
import { TaskAgentThreadDialog } from "./task-agent-thread-dialog";
import { TranscriptButton } from "../../common/task-transcript";

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
  const [localOpen, setLocalOpen] = useState(false);
  const open = controlledOpen ?? localOpen;
  const setOpen = onOpenChange ?? setLocalOpen;
	if (!task.workspace_id) {
		return (
			<TranscriptButton
				task={task}
				agentName=""
				isLive={task.status === "running"}
				title={title}
				className={className}
				renderButton={renderButton}
				open={controlledOpen}
				onOpenChange={onOpenChange}
			/>
		);
	}
  return (
    <>
      {renderButton ? (
        <Tooltip>
          <TooltipTrigger render={<button type="button" onClick={() => setOpen(true)} aria-label={title} className={cn("flex items-center justify-center rounded p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground", className)} />}>
            <MessagesSquare className="size-3.5" />
          </TooltipTrigger>
          <TooltipContent>{title}</TooltipContent>
        </Tooltip>
      ) : null}
      <TaskAgentThreadDialog workspaceId={task.workspace_id} taskId={task.id} open={open} onOpenChange={setOpen} />
    </>
  );
}
