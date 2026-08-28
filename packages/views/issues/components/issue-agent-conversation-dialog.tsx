"use client";

import { useCallback, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { MessageSquare } from "lucide-react";
import { toast } from "sonner";
import { useAuthStore } from "@patchbay/core/auth";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { useCreateComment } from "@patchbay/core/issues/mutations";
import { issueTimelineOptions } from "@patchbay/core/issues/queries";
import { unhandledCommentTriggerOutcomes } from "@patchbay/core/issues/comment-trigger-outcomes";
import { runtimeListOptions } from "@patchbay/core/runtimes/queries";
import { agentDetailOptions } from "@patchbay/core/workspace/queries";
import { api } from "@patchbay/core/api";
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
  const { mutateAsync: createComment, isPending: isSending } = useCreateComment(issueId);
  const agentName = agent?.name || t(($) => $.agent_live.fallback_name);
  const supportsGoal = runtimes.some(
    (runtime) => runtime.id === agent?.runtime_id && runtime.provider === "codex",
  );
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
  const handleSend = useCallback(
    async (
      content: string,
      attachmentIds: string[] | undefined,
      commitInput: () => void,
    ) => {
      if (!content.trim() || isSending) return false;
      const mention = `[@${escapeMarkdownLabel(agentName)}](mention://agent/${agentId})`;
      try {
        const comment = await createComment({
          content: `${mention}\n\n${content.trim()}`,
          parentId: activeMainTask ? sideChatRootId : undefined,
          attachmentIds,
        });
        commitInput();
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
        return true;
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
            onStop={() => void handleStop()}
            isRunning={!!conversation.pendingTask?.task_id}
            allowSubmitWhileRunning
            agentName={agentName}
            draftKeyOverride={`issue-agent:${issueId}:${agentId}`}
            editorKeyOverride={`issue-agent:${issueId}:${agentId}`}
          />
        </div>
      </DialogContent>
    </Dialog>
  );
}
