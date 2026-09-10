"use client";

import { useCallback, useEffect, useMemo, useRef, type ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { api, ApiError, clientErrorMessage } from "@orvilo/core/api";
import {
  agentThreadOptions,
  deriveAgentThreadTaskState,
  useContinueAgentThread,
  useSteerAgentThread,
} from "@orvilo/core/agent-thread";
import { chatKeys, unionTaskMessagesBySeq } from "@orvilo/core/chat/queries";
import { createSafeId } from "@orvilo/core/utils";
import type { AgentAvailability } from "@orvilo/core/agents";
import type { ChatMessage, TaskMessagePayload } from "@orvilo/core/types";
import { useT } from "../../i18n";
import { buildTaskAgentThreadMessages } from "../task-agent-thread";
import { AgentThreadSurface } from "./agent-thread-surface";

export function TaskAgentThreadPanel({
  workspaceId,
  taskId,
  onClose,
  title,
  unavailableReason,
  closeIcon,
  compact = false,
}: {
  workspaceId: string;
  taskId: string | null | undefined;
  onClose: () => void;
  title?: ReactNode;
  unavailableReason?: ReactNode;
  closeIcon?: "panel" | "back";
  compact?: boolean;
}) {
  const { t } = useT("issues");
  const queryClient = useQueryClient();
  const continuation = useContinueAgentThread(workspaceId);
  const steer = useSteerAgentThread(workspaceId, taskId ?? "");
  const pendingSendRef = useRef<{
    content: string;
    attachmentIds: string[];
    idempotencyKey: string;
  } | null>(null);
  const query = useQuery({
    ...agentThreadOptions(workspaceId, taskId ?? ""),
    enabled: !!taskId && !!workspaceId,
  });
  const tasks = useMemo(
    () => query.data?.thread_tasks?.length
      ? query.data.thread_tasks
      : query.data?.task ? [query.data.task] : [],
    [query.data],
  );
  const messages = useMemo<ChatMessage[]>(
    () => tasks.flatMap((task) =>
      buildTaskAgentThreadMessages(task, t(($) => $.agent_thread.task_initial_prompt))),
    [tasks, t],
  );
  const state = useMemo(() => deriveAgentThreadTaskState(tasks), [tasks]);

  useEffect(() => {
    if (!query.data?.events) return;
    const byTask = new Map<string, TaskMessagePayload[]>();
    for (const event of query.data.events) {
      byTask.set(event.task_id, [...(byTask.get(event.task_id) ?? []), event]);
    }
    for (const [eventTaskId, events] of byTask) {
      queryClient.setQueryData<TaskMessagePayload[]>(
        chatKeys.taskMessages(eventTaskId),
        (existing) => unionTaskMessagesBySeq(existing, events),
      );
    }
  }, [query.data?.events, queryClient]);

  const localizedReason = useCallback((code?: string, serverReason?: string) => {
    switch (code) {
      case "provider_session_retired": return t(($) => $.agent_thread.reason_provider_session_retired);
      case "provider_session_missing": return t(($) => $.agent_thread.reason_provider_session_missing);
      case "fresh_session_required": return t(($) => $.agent_thread.reason_fresh_session_required);
      case "provider_session_not_established": return t(($) => $.agent_thread.reason_provider_session_not_established);
      case "agent_archived": return t(($) => $.agent_thread.reason_agent_archived);
      case "agent_runtime_unbound": return t(($) => $.agent_thread.reason_agent_runtime_unbound);
      case "agent_runtime_rebound": return t(($) => $.agent_thread.reason_agent_runtime_rebound);
      case "agent_runtime_missing": return t(($) => $.agent_thread.reason_agent_runtime_missing);
      case "agent_thread_invoke_forbidden": return t(($) => $.agent_thread.reason_agent_thread_invoke_forbidden);
      case "agent_thread_depth_limit": return t(($) => $.agent_thread.reason_agent_thread_depth_limit);
      default: return serverReason || t(($) => $.agent_thread.task_continue_failed);
    }
  }, [t]);

  const handleSend = useCallback(async (
    content: string,
    attachmentIds: string[] | undefined,
    commitInput: () => void,
  ) => {
    const normalized = content.trim();
    const normalizedAttachmentIds = [...new Set(attachmentIds ?? [])].sort();
    const parentTaskId = query.data?.current_task_id ?? taskId;
    if ((!normalized && normalizedAttachmentIds.length === 0) || !parentTaskId) return false;
    const previous = pendingSendRef.current;
    const sameAttachments = previous?.attachmentIds.length === normalizedAttachmentIds.length &&
      previous.attachmentIds.every((id, index) => id === normalizedAttachmentIds[index]);
    const idempotencyKey = previous?.content === normalized && sameAttachments
      ? previous.idempotencyKey
      : createSafeId();
    pendingSendRef.current = {
      content: normalized,
      attachmentIds: normalizedAttachmentIds,
      idempotencyKey,
    };
    try {
      await continuation.mutateAsync({
        taskId: parentTaskId,
        request: {
          content: normalized,
          attachment_ids: normalizedAttachmentIds.length > 0
            ? normalizedAttachmentIds
            : undefined,
          idempotency_key: idempotencyKey,
        },
      });
      pendingSendRef.current = null;
      commitInput();
      return true;
    } catch (error) {
      if (error instanceof ApiError && (error.status === 403 || error.status === 409)) {
        const body = error.body && typeof error.body === "object"
          ? error.body as Record<string, unknown> : undefined;
        toast.error(localizedReason(
          typeof body?.reason_code === "string" ? body.reason_code : undefined,
          typeof body?.reason === "string" ? body.reason : undefined,
        ));
      } else {
        toast.error(clientErrorMessage(error) || t(($) => $.agent_thread.task_continue_failed));
      }
      return false;
    }
  }, [continuation, localizedReason, query.data?.current_task_id, t, taskId]);

  const handleStop = useCallback(async () => {
    const id = state.executingTask?.id ?? state.pendingTask?.task_id;
    if (!id) return;
    try {
      await api.cancelTaskById(id);
    } catch (error) {
      toast.error(clientErrorMessage(error) || t(($) => $.agent_thread.cancel_failed));
    }
  }, [state.executingTask?.id, state.pendingTask?.task_id, t]);

  const handleSteer = useCallback(async (queuedTaskId: string) => {
    try {
      await steer.mutateAsync(queuedTaskId);
    } catch (error) {
      if (error instanceof ApiError && error.status === 403) {
        toast.error(localizedReason("agent_thread_invoke_forbidden"));
      } else if (error instanceof ApiError && error.status === 409) {
        const body = error.body && typeof error.body === "object"
          ? error.body as Record<string, unknown> : undefined;
        toast.error(
          typeof body?.reason_code === "string" || typeof body?.reason === "string"
            ? localizedReason(
              typeof body?.reason_code === "string" ? body.reason_code : undefined,
              typeof body?.reason === "string" ? body.reason : undefined,
            )
            : clientErrorMessage(error) || t(($) => $.agent_thread.steer_unavailable),
        );
      } else {
        toast.error(clientErrorMessage(error) || t(($) => $.agent_thread.steer_failed));
      }
    }
  }, [localizedReason, steer, t]);

  const reason = unavailableReason || (query.error
    ? clientErrorMessage(query.error) || t(($) => $.agent_thread.task_load_failed)
    : query.data && !query.data.can_continue
      ? localizedReason(query.data.availability.reason_code, query.data.availability.reason)
      : !taskId
        ? t(($) => $.agent_thread.task_missing)
        : undefined);
  const availability: AgentAvailability | undefined = query.data
    ? query.data.availability.state === "available" ? "online" : "offline"
    : undefined;
  const agentName = query.data?.agent.name || t(($) => $.agent_live.fallback_name);

  return (
    <AgentThreadSurface
      onClose={onClose}
      collapseLabel={t(($) => $.agent_thread.collapse)}
      agentId={query.data?.agent.id ?? ""}
      agentName={agentName}
      title={title ?? t(($) => $.agent_thread.task_title, { name: agentName })}
      description={t(($) => $.agent_thread.task_description)}
      messages={messages}
      pendingTask={state.pendingTask}
      queueTasks={state.queuedTasks}
      availability={availability}
      isLoading={!!taskId && query.isPending}
      unavailableReason={reason}
      allowSubmitWhileRunning
      onSend={handleSend}
      onStop={() => void handleStop()}
      onSendQueuedTaskNow={handleSteer}
      draftKey={taskId ? `agent-thread:${taskId}` : undefined}
      editorKey={taskId ? `agent-thread:${taskId}` : undefined}
      closeIcon={closeIcon}
      compact={compact}
      uploadEnabled={!reason && !!query.data?.can_continue}
      checkpointLabel={[...tasks].reverse().find((task) => task.branch_name)?.branch_name}
    />
  );
}
