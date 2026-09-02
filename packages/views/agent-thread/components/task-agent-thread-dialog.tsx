"use client";

import { useCallback, useEffect, useMemo, useRef } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { api, ApiError, clientErrorMessage } from "@patchbay/core/api";
import { agentThreadOptions, deriveAgentThreadTaskState, useContinueAgentThread } from "@patchbay/core/agent-thread";
import { chatKeys, unionTaskMessagesBySeq } from "@patchbay/core/chat/queries";
import { createSafeId } from "@patchbay/core/utils";
import type { AgentAvailability } from "@patchbay/core/agents";
import type { ChatMessage, TaskMessagePayload } from "@patchbay/core/types";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@patchbay/ui/components/ui/dialog";
import { ActorAvatar } from "../../common/actor-avatar";
import { ChatInput } from "../../chat/components/chat-input";
import { ChatMessageList, ChatMessageSkeleton } from "../../chat/components/chat-message-list";
import { useT } from "../../i18n";
import { buildTaskAgentThreadMessages } from "../task-agent-thread";

export function TaskAgentThreadDialog({
  workspaceId,
  taskId,
  open,
  onOpenChange,
}: {
  workspaceId: string;
  taskId: string | null | undefined;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useT("issues");
  const queryClient = useQueryClient();
  const continuation = useContinueAgentThread(workspaceId);
  const pendingSendRef = useRef<{ content: string; idempotencyKey: string } | null>(null);
  const query = useQuery({
    ...agentThreadOptions(workspaceId, taskId ?? ""),
    enabled: open && !!taskId && !!workspaceId,
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
    _attachmentIds: string[] | undefined,
    commitInput: () => void,
  ) => {
    const normalized = content.trim();
    const parentTaskId = query.data?.current_task_id ?? taskId;
    if (!normalized || !parentTaskId) return false;
    const previous = pendingSendRef.current;
    const idempotencyKey = previous?.content === normalized
      ? previous.idempotencyKey
      : createSafeId();
    pendingSendRef.current = { content: normalized, idempotencyKey };
    try {
      await continuation.mutateAsync({
        taskId: parentTaskId,
        request: { content: normalized, idempotency_key: idempotencyKey },
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

  const reason = query.error
    ? clientErrorMessage(query.error) || t(($) => $.agent_thread.task_load_failed)
    : query.data && !query.data.can_continue
      ? localizedReason(query.data.availability.reason_code, query.data.availability.reason)
      : undefined;
  const availability: AgentAvailability | undefined = query.data
    ? query.data.availability.state === "available" ? "online" : "offline"
    : undefined;
  const agentName = query.data?.agent.name || t(($) => $.agent_live.fallback_name);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex h-[min(52rem,90svh)] w-[min(52rem,calc(100vw-2rem))] max-w-none flex-col gap-0 overflow-hidden p-0">
        <DialogHeader className="shrink-0 border-b px-5 py-3.5 text-left">
          <div className="flex items-start gap-3 pr-8">
            <ActorAvatar actorType="agent" actorId={query.data?.agent.id ?? ""} size="md" enableHoverCard />
            <div className="min-w-0 flex-1">
              <DialogTitle>{t(($) => $.agent_thread.task_title, { name: agentName })}</DialogTitle>
              <DialogDescription>{t(($) => $.agent_thread.task_description)}</DialogDescription>
            </div>
          </div>
        </DialogHeader>
        <div className="flex min-h-0 flex-1 flex-col @container">
          {query.isPending ? <ChatMessageSkeleton /> : (
            <ChatMessageList
              messages={messages}
              pendingTask={state.pendingTask}
              availability={availability}
              quickActionsDisabled
            />
          )}
          {reason ? (
            <div role="alert" className="mx-4 mb-4 rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2 text-caption text-destructive">
              {reason}
            </div>
          ) : query.data?.can_continue ? (
            <ChatInput
              onSend={handleSend}
              onStop={() => void handleStop()}
              isRunning={!!state.pendingTask?.task_id}
              allowSubmitWhileRunning
              agentName={agentName}
              leftAdornment={<ActorAvatar actorType="agent" actorId={query.data.agent.id} size="lg" profileLink={false} enableHoverCard />}
              draftKeyOverride={taskId ? `agent-thread:${taskId}` : undefined}
              editorKeyOverride={taskId ? `agent-thread:${taskId}` : undefined}
            />
          ) : null}
        </div>
      </DialogContent>
    </Dialog>
  );
}
