"use client";

import { useCallback, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { MessageSquare } from "lucide-react";
import { toast } from "sonner";
import { useAuthStore } from "@patchbay/core/auth";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { useCreateComment } from "@patchbay/core/issues/mutations";
import { issueKeys, issueTimelineOptions } from "@patchbay/core/issues/queries";
import { unhandledCommentTriggerOutcomes } from "@patchbay/core/issues/comment-trigger-outcomes";
import { runtimeListOptions } from "@patchbay/core/runtimes/queries";
import { agentDetailOptions } from "@patchbay/core/workspace/queries";
import { api } from "@patchbay/core/api";
import { useChatStore } from "@patchbay/core/chat";
import type { AgentTask } from "@patchbay/core/types";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@patchbay/ui/components/ui/dialog";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@patchbay/ui/components/ui/tooltip";
import { ActorAvatar } from "../../common/actor-avatar";
import { ChatInput } from "../../chat/components/chat-input";
import { ChatQueue } from "../../chat/components/chat-queue";
import {
  ChatMessageList,
  ChatMessageSkeleton,
} from "../../chat/components/chat-message-list";
import { escapeMarkdownLabel } from "../../editor/utils/escape-markdown-label";
import { useT } from "../../i18n";
import { buildIssueAgentConversation } from "./issue-agent-conversation";

const SIDE_CHAT_MAIN_STATUSES = new Set<AgentTask["status"]>([
  "dispatched",
  "running",
  "waiting_local_directory",
]);

/**
 * Posts a comment that mentions this agent. While a main run is live the
 * comment opens (or continues) a Side Chat; otherwise it starts a new run.
 * Steer cancels the live main task first, then posts a top-level comment.
 */
export function useIssueAgentMessageSend({
  issueId,
  agentId,
  agentName,
  tasks,
}: {
  issueId: string;
  agentId: string;
  agentName: string;
  tasks: readonly AgentTask[];
}) {
  const { t } = useT("issues");
  const { mutateAsync: createComment, isPending: isSending } =
    useCreateComment(issueId);
  const agentTasks = useMemo(
    () => tasks.filter((candidate) => candidate.agent_id === agentId),
    [agentId, tasks],
  );
  const activeMainTask = useMemo(
    () =>
      agentTasks
        .filter(
          (task) =>
            SIDE_CHAT_MAIN_STATUSES.has(task.status) &&
            !task.side_chat_parent_task_id,
        )
        .toSorted(
          (a, b) =>
            new Date(b.started_at ?? b.created_at).getTime() -
            new Date(a.started_at ?? a.created_at).getTime(),
        )[0],
    [agentTasks],
  );
  const persistedSideChatRootId = useMemo(() => {
    if (!activeMainTask) return undefined;
    return agentTasks
      .filter(
        (task) =>
          task.side_chat_parent_task_id === activeMainTask.id &&
          task.side_chat_root_comment_id,
      )
      .toSorted(
        (a, b) =>
          new Date(b.created_at).getTime() - new Date(a.created_at).getTime(),
      )[0]?.side_chat_root_comment_id;
  }, [activeMainTask, agentTasks]);
  const [localSideChat, setLocalSideChat] = useState<{
    mainTaskId: string;
    rootCommentId: string;
  } | null>(null);
  const sideChatRootId =
    localSideChat && localSideChat.mainTaskId === activeMainTask?.id
      ? localSideChat.rootCommentId
      : persistedSideChatRootId;

  const send = useCallback(
    async (
      content: string,
      attachmentIds?: string[],
      suppressAgentIds?: string[],
    ): Promise<string | false> => {
      if (!content.trim() || isSending) return false;
      const mention = `[@${escapeMarkdownLabel(agentName)}](mention://agent/${agentId})`;
      const suppress = suppressAgentIds?.filter((id) => id !== agentId);
      try {
        const comment = await createComment({
          content: `${mention}\n\n${content.trim()}`,
          parentId: activeMainTask ? sideChatRootId : undefined,
          attachmentIds,
          suppressAgentIds: suppress && suppress.length > 0 ? suppress : undefined,
        });
        const openedSideChat = comment.trigger_outcomes?.some(
          (outcome) =>
            outcome.target_type === "agent" &&
            outcome.target_id === agentId &&
            outcome.status === "side_chat",
        );
        if (openedSideChat && activeMainTask) {
          setLocalSideChat({
            mainTaskId: activeMainTask.id,
            rootCommentId: comment.parent_id || comment.id,
          });
        }
        const missedTarget = unhandledCommentTriggerOutcomes(
          comment.trigger_outcomes,
        ).some(
          (outcome) =>
            outcome.target_type === "agent" && outcome.target_id === agentId,
        );
        if (missedTarget) {
          toast.warning(
            t(($) => $.execution_log.conversation_not_triggered, {
              name: agentName,
            }),
          );
        }
        return comment.id;
      } catch (error) {
        toast.error(
          error instanceof Error && error.message
            ? error.message
            : t(($) => $.execution_log.conversation_send_failed),
        );
        return false;
      }
    },
    [
      activeMainTask,
      agentId,
      agentName,
      createComment,
      isSending,
      sideChatRootId,
      t,
    ],
  );

  const steer = useCallback(
    async (
      content: string,
      attachmentIds?: string[],
    ): Promise<string | false> => {
      if (!content.trim() || isSending) return false;
      if (activeMainTask) {
        try {
          await api.cancelTask(issueId, activeMainTask.id);
        } catch (error) {
          toast.error(
            error instanceof Error && error.message
              ? error.message
              : t(($) => $.execution_log.cancel_failed),
          );
          return false;
        }
      }
      const mention = `[@${escapeMarkdownLabel(agentName)}](mention://agent/${agentId})`;
      try {
        const comment = await createComment({
          content: `${mention}\n\n${content.trim()}`,
          attachmentIds,
        });
        const missedTarget = unhandledCommentTriggerOutcomes(
          comment.trigger_outcomes,
        ).some(
          (outcome) =>
            outcome.target_type === "agent" && outcome.target_id === agentId,
        );
        if (missedTarget) {
          toast.warning(
            t(($) => $.execution_log.conversation_not_triggered, {
              name: agentName,
            }),
          );
        }
        return comment.id;
      } catch (error) {
        toast.error(t(($) => $.execution_log.conversation_steer_send_failed));
        return false;
      }
    },
    [activeMainTask, agentId, agentName, createComment, isSending, issueId, t],
  );

  return { send, steer, isSending };
}

export function useIssueAgentTasks(issueId: string) {
  const { data: tasks = [] } = useQuery({
    queryKey: issueKeys.tasks(issueId),
    queryFn: () => api.listTasksByIssue(issueId),
    staleTime: 30_000,
    refetchOnWindowFocus: true,
  });
  return tasks;
}

export function IssueAgentConversationTrigger({
  onClick,
}: {
  onClick: () => void;
}) {
  const { t } = useT("issues");
  const title = t(($) => $.execution_log.conversation_tooltip);

  return (
    <Tooltip>
      <TooltipTrigger
        render={<button type="button" />}
        onClick={(event) => {
          event.preventDefault();
          event.stopPropagation();
          onClick();
        }}
        aria-label={title}
        className="flex items-center justify-center rounded p-1 text-muted-foreground transition-colors hover:bg-accent/50 hover:text-foreground"
      >
        <MessageSquare className="h-3.5 w-3.5" />
      </TooltipTrigger>
      <TooltipContent>{title}</TooltipContent>
    </Tooltip>
  );
}

export function IssueAgentConversationOpener({
  issueId,
  agentId,
  onOpenChange,
}: {
  issueId: string;
  agentId: string;
  onOpenChange: (open: boolean) => void;
}) {
  const tasks = useIssueAgentTasks(issueId);
  return (
    <IssueAgentConversationDialog
      issueId={issueId}
      agentId={agentId}
      tasks={tasks}
      onOpenChange={onOpenChange}
    />
  );
}

export function IssueAgentConversationDialog({
  issueId,
  agentId,
  tasks,
  onOpenChange,
}: {
  issueId: string;
  agentId: string;
  tasks: AgentTask[];
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useT("issues");
  const wsId = useWorkspaceId();
  const user = useAuthStore((state) => state.user);
  const { data: agent } = useQuery(agentDetailOptions(wsId, agentId));
  const { data: runtimes = [] } = useQuery(runtimeListOptions(wsId));
  const { data: timeline = [], isLoading } = useQuery(issueTimelineOptions(issueId));
  const agentName = agent?.name || t(($) => $.agent_live.fallback_name);
  const { send, steer } = useIssueAgentMessageSend({
    issueId,
    agentId,
    agentName,
    tasks,
  });
  const supportsGoal = runtimes.some(
    (runtime) => runtime.id === agent?.runtime_id && runtime.provider === "codex",
  );
  const agentTasks = useMemo(
    () => tasks.filter((candidate) => candidate.agent_id === agentId),
    [agentId, tasks],
  );
  const conversation = useMemo(
    () =>
      buildIssueAgentConversation({
        issueId,
        agentId,
        tasks: agentTasks,
        timeline,
        initialRunPrompt: t(($) => $.execution_log.conversation_initial_prompt),
      }),
    [agentId, agentTasks, issueId, t, timeline],
  );
  const currentTask = conversation.pendingTask?.task_id
    ? agentTasks.find(
        (candidate) => candidate.id === conversation.pendingTask?.task_id,
      )
    : undefined;
  const draftKey = `issue-agent:${issueId}:${agentId}`;
  const queuedFollowUps = useMemo(
    () =>
      (conversation.pendingTask?.queued_tasks ?? []).filter(
        (task) => task.status === "queued",
      ),
    [conversation.pendingTask?.queued_tasks],
  );

  const handleSend = useCallback(
    async (
      content: string,
      attachmentIds: string[] | undefined,
      commitInput: () => void,
    ) => {
      const commentId = await send(content, attachmentIds);
      if (!commentId) return false;
      commitInput();
      return true;
    },
    [send],
  );

  const handleSteer = useCallback(
    async (
      content: string,
      attachmentIds: string[] | undefined,
      commitInput: () => void,
    ) => {
      const commentId = await steer(content, attachmentIds);
      if (!commentId) return false;
      commitInput();
      return true;
    },
    [steer],
  );

  const handleStop = useCallback(async () => {
    if (!currentTask) return;
    try {
      await api.cancelTask(issueId, currentTask.id);
    } catch (error) {
      toast.error(
        error instanceof Error && error.message
          ? error.message
          : t(($) => $.execution_log.cancel_failed),
      );
    }
  }, [currentTask, issueId, t]);

  const cancelQueuedFollowUp = useCallback(
    async (taskId: string) => {
      try {
        await api.cancelTask(issueId, taskId);
        return true;
      } catch (error) {
        toast.error(
          error instanceof Error && error.message
            ? error.message
            : t(($) => $.execution_log.cancel_failed),
        );
        return false;
      }
    },
    [issueId, t],
  );

  const handleEditQueuedFollowUp = useCallback(
    async (taskId: string) => {
      const queued = queuedFollowUps.find((task) => task.task_id === taskId);
      const cancelled = await cancelQueuedFollowUp(taskId);
      if (!cancelled || !queued?.content?.trim()) return;
      useChatStore.getState().setInputDraft(draftKey, queued.content);
    },
    [cancelQueuedFollowUp, draftKey, queuedFollowUps],
  );

  const handleClearQueuedFollowUps = useCallback(async () => {
    for (const task of queuedFollowUps) {
      const cancelled = await cancelQueuedFollowUp(task.task_id);
      if (!cancelled) return;
    }
  }, [cancelQueuedFollowUp, queuedFollowUps]);

  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent className="flex h-[min(52rem,90svh)] w-[min(52rem,calc(100vw-2rem))] max-w-none flex-col gap-0 overflow-hidden p-0">
        <DialogHeader className="shrink-0 border-b px-5 py-3.5 text-left">
          <div className="flex items-start gap-3 pr-8">
            <ActorAvatar
              actorType="agent"
              actorId={agentId}
              size="md"
              enableHoverCard
            />
            <div className="min-w-0 flex-1">
              <DialogTitle className="truncate text-body">
                {t(($) => $.execution_log.conversation_title, { name: agentName })}
              </DialogTitle>
              <DialogDescription className="mt-0.5 text-caption">
                <span className="block">
                  {t(($) => $.execution_log.conversation_description, {
                    count: agentTasks.length,
                  })}
                </span>
                {supportsGoal && (
                  <span className="mt-0.5 block">
                    {t(($) => $.execution_log.conversation_goal_hint)}
                  </span>
                )}
              </DialogDescription>
            </div>
          </div>
        </DialogHeader>

        <div className="flex min-h-0 flex-1 flex-col @container">
          {isLoading ? (
            <ChatMessageSkeleton />
          ) : (
            <ChatMessageList
              messages={conversation.messages}
              messageActors={conversation.messageActors}
              agentId={agentId}
              agentName={agentName}
              userId={user?.id}
              userName={user?.name}
              pendingTask={conversation.pendingTask}
              availability={undefined}
              quickActionsDisabled
            />
          )}
          <ChatInput
            onSend={handleSend}
            onSteer={handleSteer}
            onStop={() => void handleStop()}
            isRunning={!!conversation.pendingTask?.task_id}
            allowSubmitWhileRunning
            chooseFollowUp
            queueSlot={
              <ChatQueue
                tasks={queuedFollowUps}
                headStatus={conversation.pendingTask?.status}
                onEdit={handleEditQueuedFollowUp}
                onRemove={(taskId) => void cancelQueuedFollowUp(taskId)}
                onClear={handleClearQueuedFollowUps}
              />
            }
            agentName={agentName}
            leftAdornment={
              <ActorAvatar
                actorType="agent"
                actorId={agentId}
                size="lg"
                profileLink={false}
                enableHoverCard
              />
            }
            draftKeyOverride={draftKey}
            editorKeyOverride={draftKey}
          />
        </div>
      </DialogContent>
    </Dialog>
  );
}
