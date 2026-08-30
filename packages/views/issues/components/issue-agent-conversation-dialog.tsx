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
import { attachmentToDraftUpload } from "@patchbay/core/drafts";
import { useChatStore } from "@patchbay/core/chat";
import type { AgentTask } from "@patchbay/core/types";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@patchbay/ui/components/ui/tooltip";
import { AgentThreadSurface } from "../../agent-thread";
import { escapeMarkdownLabel } from "../../editor/utils/escape-markdown-label";
import { useT } from "../../i18n";
import { stripMentionMarkdown } from "../utils/strip-mention-markdown";
import {
  buildIssueAgentConversation,
  queuedIssueFollowUps,
} from "./issue-agent-conversation";

const SIDE_CHAT_MAIN_STATUSES = new Set<AgentTask["status"]>([
  "dispatched",
  "running",
  "waiting_local_directory",
]);

const LIVE_FOLLOW_UP_STATUSES = new Set<AgentTask["status"]>([
  "dispatched",
  "running",
  "waiting_local_directory",
  "queued",
  "deferred",
]);

/**
 * Posts a comment that mentions this agent. While a main run is live the
 * comment opens (or continues) a Side Chat; otherwise it starts a new run.
 * Steer cancels every live and queued run for this agent, then posts a
 * top-level comment so a Side Chat reply cannot keep going in the background.
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
          suppressAgentIds:
            suppress && suppress.length > 0 ? suppress : undefined,
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
            t(($) => $.agent_thread.conversation_not_triggered, {
              name: agentName,
            }),
          );
        }
        return comment.id;
      } catch (error) {
        toast.error(
          error instanceof Error && error.message
            ? error.message
            : t(($) => $.agent_thread.conversation_send_failed),
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
      const liveTasks = agentTasks.filter((task) =>
        LIVE_FOLLOW_UP_STATUSES.has(task.status),
      );
      for (const task of liveTasks) {
        try {
          await api.cancelTask(issueId, task.id);
        } catch (error) {
          toast.error(
            error instanceof Error && error.message
              ? error.message
              : t(($) => $.agent_thread.cancel_failed),
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
            t(($) => $.agent_thread.conversation_not_triggered, {
              name: agentName,
            }),
          );
        }
        return comment.id;
      } catch (error) {
        toast.error(t(($) => $.agent_thread.conversation_steer_send_failed));
        return false;
      }
    },
    [agentId, agentName, agentTasks, createComment, isSending, issueId, t],
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
  const title = t(($) => $.agent_thread.conversation_tooltip);

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
  const { data: timeline = [], isLoading } = useQuery(
    issueTimelineOptions(issueId),
  );
  const agentName = agent?.name || t(($) => $.agent_live.fallback_name);
  const { send, steer } = useIssueAgentMessageSend({
    issueId,
    agentId,
    agentName,
    tasks,
  });
  const supportsGoal = runtimes.some(
    (runtime) =>
      runtime.id === agent?.runtime_id && runtime.provider === "codex",
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
        initialRunPrompt: t(($) => $.agent_thread.conversation_initial_prompt),
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
    () => queuedIssueFollowUps(conversation.pendingTask, agentTasks),
    [agentTasks, conversation.pendingTask],
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
          : t(($) => $.agent_thread.cancel_failed),
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
            : t(($) => $.agent_thread.cancel_failed),
        );
        return false;
      }
    },
    [issueId, t],
  );

  const handleEditQueuedFollowUp = useCallback(
    async (taskId: string) => {
      const sourceTask = agentTasks.find(
        (candidate) => candidate.id === taskId,
      );
      const comment = sourceTask?.trigger_comment_id
        ? timeline.find(
            (entry) =>
              entry.type === "comment" &&
              entry.id === sourceTask.trigger_comment_id,
          )
        : undefined;
      const rawContent = comment?.content?.trim()
        ? comment.content.includes("\n\n")
          ? comment.content.slice(comment.content.indexOf("\n\n") + 2)
          : comment.content
        : (sourceTask?.trigger_summary ?? "");
      const content = stripMentionMarkdown(rawContent).trim();
      const attachments = comment?.attachments ?? [];
      if (content) {
        useChatStore.getState().setInputDraft(draftKey, content);
      }
      if (attachments.length > 0) {
        useChatStore
          .getState()
          .setInputDraftAttachments(
            draftKey,
            attachments.map(attachmentToDraftUpload),
          );
      }
      await cancelQueuedFollowUp(taskId);
    },
    [agentTasks, cancelQueuedFollowUp, draftKey, timeline],
  );

  const handleClearQueuedFollowUps = useCallback(async () => {
    for (const task of queuedFollowUps) {
      const cancelled = await cancelQueuedFollowUp(task.task_id);
      if (!cancelled) return;
    }
  }, [cancelQueuedFollowUp, queuedFollowUps]);

  return (
    <AgentThreadSurface
      open
      onOpenChange={onOpenChange}
      agentId={agentId}
      agentName={agentName}
      userId={user?.id}
      userName={user?.name}
      title={t(($) => $.agent_thread.conversation_title, { name: agentName })}
      description={t(($) => $.agent_thread.conversation_description, {
        count: agentTasks.length,
      })}
      descriptionHint={
        supportsGoal
          ? t(($) => $.agent_thread.conversation_goal_hint)
          : undefined
      }
      messages={conversation.messages}
      messageActors={conversation.messageActors}
      pendingTask={conversation.pendingTask}
      availability={undefined}
      isLoading={isLoading}
      quickActionsDisabled
      allowSubmitWhileRunning
      chooseFollowUp
      onSend={handleSend}
      onSteer={handleSteer}
      onStop={() => void handleStop()}
      queueTasks={queuedFollowUps}
      onEditQueuedTask={handleEditQueuedFollowUp}
      onRemoveQueuedTask={async (taskId) => {
        await cancelQueuedFollowUp(taskId);
      }}
      onClearQueuedTasks={handleClearQueuedFollowUps}
      draftKey={draftKey}
      editorKey={draftKey}
    />
  );
}
