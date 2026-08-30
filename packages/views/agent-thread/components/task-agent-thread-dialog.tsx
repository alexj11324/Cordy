"use client";

import type { ReactNode } from "react";
import { useCallback, useEffect, useMemo } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useAuthStore } from "@patchbay/core/auth";
import { api, clientErrorMessage } from "@patchbay/core/api";
import { chatKeys, unionTaskMessagesBySeq } from "@patchbay/core/chat/queries";
import { useContinueAgentThread } from "@patchbay/core/agent-thread";
import { agentThreadOptions } from "@patchbay/core/agent-thread/queries";
import { useWorkspaceId } from "@patchbay/core/hooks";
import type { AgentAvailability } from "@patchbay/core/agents";
import type { ChatMessage, TaskMessagePayload } from "@patchbay/core/types";
import { toast } from "sonner";
import { useT } from "../../i18n";
import { AgentThreadSurface } from "./agent-thread-surface";
import {
  buildTaskAgentThreadMessages,
  pendingTaskForAgentThread,
} from "../task-agent-thread";
import { createSafeId } from "@patchbay/core/utils";

export function TaskAgentThreadDialog({
  taskId,
  open,
  onOpenChange,
  title,
  unavailableReason,
}: {
  taskId: string | null | undefined;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title?: ReactNode;
  unavailableReason?: ReactNode;
}) {
  const { t } = useT("issues");
  const wsId = useWorkspaceId();
  const user = useAuthStore((state) => state.user);
  const queryClient = useQueryClient();
  const continuation = useContinueAgentThread();
  const hasTask = Boolean(taskId);
  const query = useQuery({
    ...agentThreadOptions(wsId, taskId ?? ""),
    enabled: open && hasTask,
  });
  const task = query.data?.task;
  const threadTasks = useMemo(
    () =>
      query.data?.thread_tasks?.length
        ? query.data.thread_tasks
        : task
          ? [task]
          : [],
    [query.data?.thread_tasks, task],
  );
  const continuationParentTaskId = task?.id ?? taskId;
  const agentName = query.data?.agent.name || t(($) => $.agent_live.fallback_name);
  const initialPrompt = t(($) => $.agent_thread.task_initial_prompt);
  const messages = useMemo<ChatMessage[]>(
    () =>
      threadTasks.flatMap((threadTask) =>
        buildTaskAgentThreadMessages(threadTask, initialPrompt),
      ),
    [initialPrompt, threadTasks],
  );
  const pendingTask = pendingTaskForAgentThread(task);

  // The envelope contains the same structured event stream used by the live
  // Chat renderer. Seed the canonical task-message cache so AssistantMessage
  // does not need a second request when a historical thread is opened.
  useEffect(() => {
    if (!query.data?.events) return;
    const eventsByTask = new Map<string, TaskMessagePayload[]>();
    for (const event of query.data.events) {
      const taskEvents = eventsByTask.get(event.task_id) ?? [];
      taskEvents.push(event);
      eventsByTask.set(event.task_id, taskEvents);
    }
    for (const [eventTaskId, taskEvents] of eventsByTask) {
      queryClient.setQueryData<TaskMessagePayload[]>(
        chatKeys.taskMessages(eventTaskId),
        (existing) => unionTaskMessagesBySeq(existing, taskEvents),
      );
    }
  }, [query.data?.events, queryClient]);

  const handleSend = useCallback(
    async (
      content: string,
      _attachmentIds: string[] | undefined,
      commitInput: () => void,
    ) => {
      if (!continuationParentTaskId || !content.trim()) return false;
      try {
        await continuation.mutateAsync({
          taskId: continuationParentTaskId,
          request: { content: content.trim(), idempotency_key: createSafeId() },
        });
        commitInput();
        return true;
      } catch (error) {
        toast.error(
          clientErrorMessage(error) || t(($) => $.agent_thread.task_continue_failed),
        );
        return false;
      }
    },
    [continuation, continuationParentTaskId, t],
  );

  const handleStop = useCallback(async () => {
    if (!continuationParentTaskId) return;
    try {
      await api.cancelTaskById(continuationParentTaskId);
    } catch (error) {
      toast.error(
        clientErrorMessage(error) || t(($) => $.agent_thread.cancel_failed),
      );
    }
  }, [continuationParentTaskId, t]);

  const loadError = query.error
    ? clientErrorMessage(query.error) || t(($) => $.agent_thread.task_load_failed)
    : undefined;
  const continuationUnavailableReason =
    query.data && !query.data.can_continue
      ? query.data.availability.reason || t(($) => $.agent_thread.task_continue_failed)
      : undefined;
  const terminalReason = unavailableReason ||
    (query.data?.availability.state === "unavailable"
      ? query.data.availability.reason || t(($) => $.agent_thread.task_load_failed)
      : continuationUnavailableReason || loadError) ||
    (!hasTask ? t(($) => $.agent_thread.task_missing) : undefined);
  const availability: AgentAvailability | undefined = query.data
    ? query.data.availability.state === "available"
      ? "online"
      : "offline"
    : undefined;
  return (
    <AgentThreadSurface
      open={open}
      onOpenChange={onOpenChange}
      agentId={query.data?.agent.id ?? ""}
      agentName={agentName}
      userId={user?.id}
      userName={user?.name}
      title={title ?? t(($) => $.agent_thread.task_title, { name: agentName })}
      description={t(($) => $.agent_thread.task_description)}
      messages={messages}
      pendingTask={pendingTask}
      availability={availability}
      isLoading={hasTask && query.isPending}
      unavailableReason={terminalReason}
      allowSubmitWhileRunning
      onSend={handleSend}
      onStop={() => void handleStop()}
      draftKey={taskId ? `agent-thread:${taskId}` : undefined}
      editorKey={taskId ? `agent-thread:${taskId}` : undefined}
    />
  );
}
