"use client";

import type { ReactNode } from "react";
import { useCallback, useEffect, useMemo, useRef } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useAuthStore } from "@patchbay/core/auth";
import { api, ApiError, clientErrorMessage } from "@patchbay/core/api";
import { chatKeys, unionTaskMessagesBySeq } from "@patchbay/core/chat/queries";
import { useContinueAgentThread } from "@patchbay/core/agent-thread";
import { agentThreadOptions } from "@patchbay/core/agent-thread/queries";
import { useWorkspaceId } from "@patchbay/core/hooks";
import type { AgentAvailability } from "@patchbay/core/agents";
import type { ChatMessage, TaskMessagePayload } from "@patchbay/core/types";
import { toast } from "sonner";
import { useT } from "../../i18n";
import { AgentThreadSurface } from "./agent-thread-surface";
import { buildTaskAgentThreadMessages } from "../task-agent-thread";
import { createSafeId } from "@patchbay/core/utils";
import { deriveAgentThreadTaskState } from "@patchbay/core/agent-thread";

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
  const pendingSendRef = useRef<{
    taskId: string;
    content: string;
    idempotencyKey: string;
  } | null>(null);
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
  const agentName =
    query.data?.agent.name || t(($) => $.agent_live.fallback_name);
  const initialPrompt = t(($) => $.agent_thread.task_initial_prompt);
  const messages = useMemo<ChatMessage[]>(
    () =>
      threadTasks.flatMap((threadTask) =>
        buildTaskAgentThreadMessages(threadTask, initialPrompt),
      ),
    [initialPrompt, threadTasks],
  );
  const taskState = useMemo(
    () => deriveAgentThreadTaskState(threadTasks),
    [threadTasks],
  );
  const pendingTask = taskState.pendingTask;

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

  const localizedAvailabilityReason = useCallback(
    (
      reasonCode: string | undefined,
      serverReason: string | undefined,
      fallback: string,
    ): string => {
      switch (reasonCode) {
        case "provider_session_retired":
          return t(($) => $.agent_thread.reason_provider_session_retired);
        case "provider_session_missing":
          return t(($) => $.agent_thread.reason_provider_session_missing);
        case "fresh_session_required":
          return t(($) => $.agent_thread.reason_fresh_session_required);
        case "provider_session_not_established":
          return t(
            ($) => $.agent_thread.reason_provider_session_not_established,
          );
        case "agent_archived":
          return t(($) => $.agent_thread.reason_agent_archived);
        case "agent_runtime_unbound":
          return t(($) => $.agent_thread.reason_agent_runtime_unbound);
        case "agent_runtime_rebound":
          return t(($) => $.agent_thread.reason_agent_runtime_rebound);
        case "agent_runtime_missing":
          return t(($) => $.agent_thread.reason_agent_runtime_missing);
        case "agent_thread_invoke_forbidden":
          return t(($) => $.agent_thread.reason_agent_thread_invoke_forbidden);
        default:
          return serverReason || fallback;
      }
    },
    [t],
  );

  const continuationErrorMessage = useCallback(
    (error: unknown): string => {
      if (error instanceof ApiError && error.status === 403) {
        const body =
          error.body && typeof error.body === "object"
            ? (error.body as Record<string, unknown>)
            : undefined;
        return localizedAvailabilityReason(
          typeof body?.reason_code === "string" ? body.reason_code : undefined,
          typeof body?.reason === "string" ? body.reason : undefined,
          t(($) => $.agent_thread.reason_agent_thread_invoke_forbidden),
        );
      }
      if (error instanceof ApiError && error.status === 409) {
        const body =
          error.body && typeof error.body === "object"
            ? (error.body as Record<string, unknown>)
            : undefined;
        return localizedAvailabilityReason(
          typeof body?.reason_code === "string" ? body.reason_code : undefined,
          typeof body?.reason === "string" ? body.reason : undefined,
          t(($) => $.agent_thread.task_continue_failed),
        );
      }
      return (
        clientErrorMessage(error) ||
        t(($) => $.agent_thread.task_continue_failed)
      );
    },
    [localizedAvailabilityReason, t],
  );

  const handleSend = useCallback(
    async (
      content: string,
      _attachmentIds: string[] | undefined,
      commitInput: () => void,
    ) => {
      const normalizedContent = content.trim();
      if (!continuationParentTaskId || !normalizedContent) return false;
      const pendingSend = pendingSendRef.current;
      const idempotencyKey =
        pendingSend?.taskId === continuationParentTaskId &&
        pendingSend.content === normalizedContent
          ? pendingSend.idempotencyKey
          : createSafeId();
      pendingSendRef.current = {
        taskId: continuationParentTaskId,
        content: normalizedContent,
        idempotencyKey,
      };
      try {
        await continuation.mutateAsync({
          taskId: continuationParentTaskId,
          request: {
            content: normalizedContent,
            idempotency_key: idempotencyKey,
          },
        });
        pendingSendRef.current = null;
        commitInput();
        return true;
      } catch (error) {
        toast.error(continuationErrorMessage(error));
        return false;
      }
    },
    [continuation, continuationErrorMessage, continuationParentTaskId],
  );

  const handleStop = useCallback(async () => {
    const taskToStop =
      taskState.executingTask?.id ??
      pendingTask?.task_id ??
      continuationParentTaskId;
    if (!taskToStop) return;
    try {
      await api.cancelTaskById(taskToStop);
    } catch (error) {
      toast.error(
        clientErrorMessage(error) || t(($) => $.agent_thread.cancel_failed),
      );
    }
  }, [
    continuationParentTaskId,
    pendingTask?.task_id,
    t,
    taskState.executingTask?.id,
  ]);

  const loadError = query.error
    ? clientErrorMessage(query.error) ||
      t(($) => $.agent_thread.task_load_failed)
    : undefined;
  const continuationUnavailableReason =
    query.data && !query.data.can_continue
      ? localizedAvailabilityReason(
          query.data.availability.reason_code,
          query.data.availability.reason,
          t(($) => $.agent_thread.task_continue_failed),
        )
      : undefined;
  const terminalReason =
    unavailableReason ||
    (query.data?.availability.state === "unavailable"
      ? localizedAvailabilityReason(
          query.data.availability.reason_code,
          query.data.availability.reason,
          t(($) => $.agent_thread.task_load_failed),
        )
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
      queueTasks={taskState.queuedTasks}
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
