"use client";

import { memo, useMemo, useState } from "react";
import { ThinkingOrb } from "thinking-orbs";
import { useAuthStore } from "@patchbay/core/auth";
import type { AgentTask } from "@patchbay/core/types";
import { useActorName } from "@patchbay/core/workspace/hooks";
import { cn } from "@patchbay/ui/lib/utils";
import { Card } from "@patchbay/ui/components/ui/card";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@patchbay/ui/components/ui/tooltip";
import { ActorAvatar } from "../../common/actor-avatar";
import { useT, useTimeAgo } from "../../i18n";
import {
  IssueAgentConversationDialog,
  IssueAgentConversationTrigger,
  useIssueAgentMessageSend,
  useIssueAgentTasks,
} from "./issue-agent-conversation-dialog";
import { ReplyInput } from "./reply-input";

const LIVE_STATUS_RANK: Partial<Record<AgentTask["status"], number>> = {
  running: 0,
  dispatched: 1,
  waiting_local_directory: 2,
  queued: 3,
};

function isLiveTask(task: AgentTask): boolean {
  return LIVE_STATUS_RANK[task.status] !== undefined;
}

function taskRecency(task: AgentTask): number {
  return new Date(
    task.completed_at ?? task.started_at ?? task.created_at,
  ).getTime();
}

function sortByLiveRank(a: AgentTask, b: AgentTask): number {
  const rank =
    (LIVE_STATUS_RANK[a.status] ?? 99) - (LIVE_STATUS_RANK[b.status] ?? 99);
  if (rank !== 0) return rank;
  return taskRecency(b) - taskRecency(a);
}

/** The run whose Working capsule belongs in the activity thread. */
export function pickIssueAgentLiveTask(
  tasks: readonly AgentTask[],
): AgentTask | undefined {
  return tasks.filter(isLiveTask).toSorted(sortByLiveRank)[0];
}

function IssueAgentConversationHost({
  issueId,
  task,
  tasks,
  onOpenChange,
}: {
  issueId: string;
  task: AgentTask | null;
  tasks: AgentTask[];
  onOpenChange: (open: boolean) => void;
}) {
  const resolved = task
    ? (tasks.find((candidate) => candidate.id === task.id) ?? task)
    : null;
  if (!resolved) return null;
  return (
    <IssueAgentConversationDialog
      issueId={issueId}
      agentId={resolved.agent_id}
      tasks={tasks}
      onOpenChange={onOpenChange}
    />
  );
}

function IssueAgentWorkingCard({
  issueId,
  task,
  tasks,
  onOpenConversation,
}: {
  issueId: string;
  task: AgentTask;
  tasks: AgentTask[];
  onOpenConversation: (task: AgentTask) => void;
}) {
  const { t } = useT("issues");
  const timeAgo = useTimeAgo();
  const { getActorName } = useActorName();
  const user = useAuthStore((state) => state.user);
  const agentName =
    getActorName("agent", task.agent_id) ||
    t(($) => $.agent_live.fallback_name);
  const { send } = useIssueAgentMessageSend({
    issueId,
    agentId: task.agent_id,
    agentName,
    tasks,
  });
  const running = task.status === "running";
  const status =
    task.status === "running"
      ? t(($) => $.agent_live.working)
      : task.status === "waiting_local_directory"
        ? t(($) => $.execution_log.status_waiting_local_directory)
        : t(($) => $.agent_activity.status_queued);
  const happenedAt = task.started_at ?? task.created_at;

  return (
    <div className="pb-3">
      <Card className="!gap-0 !py-0 overflow-clip">
        <div className="px-4 py-3 max-md:px-3">
          <div className="flex items-center gap-2.5">
            <ActorAvatar
              actorType="agent"
              actorId={task.agent_id}
              size="md"
              enableHoverCard
              showStatusDot
            />
            <span className="shrink-0 text-body font-medium">{agentName}</span>
            <Tooltip>
              <TooltipTrigger
                render={
                  <span className="shrink-0 cursor-default text-caption text-muted-foreground">
                    {timeAgo(happenedAt)}
                  </span>
                }
              />
              <TooltipContent side="top">
                {new Date(happenedAt).toLocaleString()}
              </TooltipContent>
            </Tooltip>
            <div className="ml-auto flex items-center gap-0.5">
              <IssueAgentConversationTrigger
                onClick={() => onOpenConversation(task)}
              />
            </div>
          </div>
          <div
            data-issue-agent-working={running ? "running" : "queued"}
            className="mt-1 flex items-center gap-2 py-0.5 pl-10 text-body text-muted-foreground max-md:pl-0"
          >
            <span className={cn(running && "animate-chat-text-shimmer")}>
              {status}
            </span>
            <ThinkingOrb
              aria-hidden
              size={20}
              state={running ? "working" : "connecting"}
              theme="auto"
            />
          </div>
        </div>
        <div className="border-t border-border/50 px-4 py-2.5 max-md:px-3">
          <ReplyInput
            issueId={issueId}
            size="sm"
            placeholder={t(($) => $.reply.placeholder)}
            avatarType="member"
            avatarId={user?.id ?? ""}
            draftKey={`reply:${issueId}:${task.id}`}
            onSubmit={(content, attachmentIds, suppressAgentIds) =>
              send(content, attachmentIds, suppressAgentIds)
            }
          />
        </div>
      </Card>
    </div>
  );
}

/**
 * Live agent turn inside the issue thread. Same card + header as a human
 * comment so the agent is a first-class participant, not a sidecar chip.
 * Body is the Working line plus the orbs `working` animation; the nested
 * reply box stays available while the agent is still running.
 */
export const IssueAgentWorkingStatus = memo(function IssueAgentWorkingStatus({
  issueId,
}: {
  issueId: string;
}) {
  const tasks = useIssueAgentTasks(issueId);
  const liveTasks = useMemo(
    () => tasks.filter(isLiveTask).toSorted(sortByLiveRank),
    [tasks],
  );
  const [opened, setOpened] = useState<AgentTask | null>(null);
  if (liveTasks.length === 0 && !opened) return null;

  return (
    <>
      {liveTasks.map((task) => (
        <IssueAgentWorkingCard
          key={task.id}
          issueId={issueId}
          task={task}
          tasks={tasks}
          onOpenConversation={setOpened}
        />
      ))}
      <IssueAgentConversationHost
        issueId={issueId}
        task={opened}
        tasks={tasks}
        onOpenChange={(open) => {
          if (!open) setOpened(null);
        }}
      />
    </>
  );
});
