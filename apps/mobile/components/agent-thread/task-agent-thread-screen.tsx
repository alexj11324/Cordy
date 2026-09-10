/**
 * The mobile Agent thread surface for a persisted task. It composes the
 * already-shipping ChatMessageList + ChatComposer; this route is the Agent
 * thread surface, not a separate task or event inspection product surface.
 */
import { useCallback, useEffect, useMemo, useRef } from "react";
import { Alert, KeyboardAvoidingView, Platform, View } from "react-native";
import { Ionicons } from "@expo/vector-icons";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { ChatMessage, TaskMessagePayload } from "@orvilo/core/types";
import { createSafeId } from "@orvilo/core/utils";
import { deriveAgentThreadTaskState } from "@orvilo/core/agent-thread";
import { api, ApiError } from "@/data/api";
import { chatKeys, taskMessagesOptions } from "@/data/queries/chat";
import { agentThreadOptions } from "@/data/queries/agent-thread";
import { useContinueAgentThread } from "@/data/mutations/agent-thread";
import { useWorkspaceStore } from "@/data/workspace-store";
import { useChatDraftsStore } from "@/data/stores/chat-drafts-store";
import { ChatMessageList } from "@/components/chat/chat-message-list";
import { ChatComposer } from "@/components/chat/chat-composer";
import { Text } from "@/components/ui/text";
import { buildAgentThreadMessages } from "@/lib/agent-thread-display";
import {
  agentThreadAvailabilityMessage,
  type AgentThreadCopy,
} from "@/lib/agent-thread-i18n";
import { useAgentThreadCopy } from "@/lib/use-agent-thread-copy";

interface Props {
  taskId: string;
}

function unionTaskMessagesBySeq(
  existing: readonly TaskMessagePayload[] | undefined,
  incoming: readonly TaskMessagePayload[],
): TaskMessagePayload[] {
  if (!existing || existing.length === 0) {
    return [...incoming].sort((a, b) => a.seq - b.seq);
  }
  const bySeq = new Map(existing.map((message) => [message.seq, message]));
  let changed = false;
  for (const message of incoming) {
    if (bySeq.get(message.seq) !== message) {
      bySeq.set(message.seq, message);
      changed = true;
    }
  }
  return changed
    ? [...bySeq.values()].sort((a, b) => a.seq - b.seq)
    : (existing as TaskMessagePayload[]);
}

function continuationError(error: unknown, copy: AgentThreadCopy): string {
  if (error instanceof ApiError && error.status === 403) {
    const body =
      error.body && typeof error.body === "object"
        ? (error.body as Record<string, unknown>)
        : undefined;
    return agentThreadAvailabilityMessage(
      copy,
      typeof body?.reason_code === "string" ? body.reason_code : undefined,
      typeof body?.reason === "string" ? body.reason : undefined,
      copy.permission_denied,
    );
  }
  if (error instanceof ApiError && error.status === 409) {
    const body =
      error.body && typeof error.body === "object"
        ? (error.body as Record<string, unknown>)
        : undefined;
    return agentThreadAvailabilityMessage(
      copy,
      typeof body?.reason_code === "string" ? body.reason_code : undefined,
      typeof body?.reason === "string" ? body.reason : undefined,
      copy.unavailable,
    );
  }
  return error instanceof Error ? error.message : copy.could_not_continue;
}

export function TaskAgentThreadScreen({ taskId }: Props) {
  const copy = useAgentThreadCopy();
  const wsId = useWorkspaceStore((state) => state.currentWorkspaceId);
  const queryClient = useQueryClient();
  const threadQuery = useQuery(agentThreadOptions(wsId, taskId));
  const continuation = useContinueAgentThread();
  const pendingSendRef = useRef<{
    content: string;
    attachmentIds: string[];
    idempotencyKey: string;
  } | null>(null);
  const draft = useChatDraftsStore(
    (state) => state.drafts[`agent-thread:${taskId}`] ?? "",
  );
  const setDraft = useChatDraftsStore((state) => state.setDraft);
  const clearDraft = useChatDraftsStore((state) => state.clearDraft);
  const task = threadQuery.data?.task;
  const threadTasks = useMemo(
    () =>
      threadQuery.data?.thread_tasks?.length
        ? threadQuery.data.thread_tasks
        : task
          ? [task]
          : [],
    [task, threadQuery.data?.thread_tasks],
  );
  const continuationParentTaskId =
    threadQuery.data?.current_task_id || task?.id || taskId;
  const messages = useMemo<ChatMessage[]>(
    () =>
      threadTasks.flatMap((threadTask) =>
        buildAgentThreadMessages(threadTask, copy.continue_prompt),
      ),
    [copy.continue_prompt, threadTasks],
  );
  const taskState = useMemo(
    () => deriveAgentThreadTaskState(threadTasks),
    [threadTasks],
  );
  const pendingTask = taskState.pendingTask;
  const liveTaskId =
    taskState.executingTask?.id ??
    pendingTask?.task_id ??
    continuationParentTaskId;
  const liveTaskMessages = useQuery(taskMessagesOptions(liveTaskId));

  useEffect(() => {
    if (!threadQuery.data?.events) return;
    const eventsByTask = new Map<string, TaskMessagePayload[]>();
    for (const event of threadQuery.data.events) {
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
  }, [queryClient, threadQuery.data?.events]);

  const handleSend = useCallback(
    async (content: string, attachmentIds: string[]) => {
      const normalizedContent = content.trim();
      const normalizedAttachmentIds = [...new Set(attachmentIds)].sort();
      if (
        !threadQuery.data?.can_continue ||
        threadQuery.data.availability.state !== "available" ||
        !continuationParentTaskId ||
        (!normalizedContent && normalizedAttachmentIds.length === 0)
      ) {
        throw new Error(
          agentThreadAvailabilityMessage(
            copy,
            threadQuery.data?.availability.reason_code,
            threadQuery.data?.availability.reason,
            copy.unavailable_fallback,
          ),
        );
      }
      const pendingSend = pendingSendRef.current;
      const sameAttachments =
        pendingSend?.attachmentIds.length === normalizedAttachmentIds.length &&
        pendingSend.attachmentIds.every(
          (id, index) => id === normalizedAttachmentIds[index],
        );
      const idempotencyKey =
        pendingSend?.content === normalizedContent && sameAttachments
          ? pendingSend.idempotencyKey
          : createSafeId();
      pendingSendRef.current = {
        content: normalizedContent,
        attachmentIds: normalizedAttachmentIds,
        idempotencyKey,
      };
      await continuation.mutateAsync({
        taskId: continuationParentTaskId,
        request: {
          content: normalizedContent,
          ...(normalizedAttachmentIds.length > 0
            ? { attachment_ids: normalizedAttachmentIds }
            : {}),
          idempotency_key: idempotencyKey,
        },
      });
      pendingSendRef.current = null;
      clearDraft(`agent-thread:${taskId}`);
    },
    [
      clearDraft,
      continuation,
      continuationParentTaskId,
      copy,
      taskId,
      threadQuery.data,
    ],
  );

  const handleStop = useCallback(() => {
    const taskToStop =
      taskState.executingTask?.id ??
      pendingTask?.task_id ??
      continuationParentTaskId;
    if (!taskToStop) return;
    void api.cancelTaskById(taskToStop).catch((error) => {
      Alert.alert(copy.unable_to_stop, continuationError(error, copy));
    });
  }, [
    continuationParentTaskId,
    copy,
    pendingTask?.task_id,
    taskState.executingTask?.id,
  ]);

  const unavailableReason = threadQuery.isError
    ? continuationError(threadQuery.error, copy)
    : threadQuery.data &&
        (!threadQuery.data.can_continue ||
          threadQuery.data.availability.state === "unavailable")
      ? agentThreadAvailabilityMessage(
          copy,
          threadQuery.data.availability.reason_code,
          threadQuery.data.availability.reason,
          copy.unavailable_fallback,
        )
      : undefined;
  const availability = threadQuery.data
    ? threadQuery.data.availability.state === "available"
      ? "online"
      : "offline"
    : undefined;
  const canContinue = Boolean(
    threadQuery.data?.can_continue &&
    threadQuery.data.availability.state === "available",
  );
  const checkpoint = [...(threadQuery.data?.thread_tasks ?? [])]
    .reverse()
    .find((task) => task.branch_name)?.branch_name;

  return (
    <View className="flex-1 bg-background">
      <KeyboardAvoidingView
        behavior={Platform.OS === "ios" ? "padding" : undefined}
        className="flex-1"
      >
        <ChatMessageList
          messages={messages}
          loading={threadQuery.isPending}
          hasSessions
          agent={null}
          onPickPrompt={(text) => setDraft(`agent-thread:${taskId}`, text)}
          pendingTask={pendingTask}
          queueTasks={taskState.queuedTasks}
          liveTaskMessages={liveTaskMessages.data ?? []}
          availability={availability}
        />
        {checkpoint ? (
          <View
            accessible
            accessibilityLabel={`${copy.checkpoint}: ${checkpoint}`}
            className="mx-3 mb-2 flex-row items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5"
          >
            <Ionicons name="git-branch-outline" size={14} color="#71717a" />
            <Text
              className="flex-1 text-xs text-muted-foreground"
              numberOfLines={1}
            >
              {checkpoint}
            </Text>
          </View>
        ) : null}
        <ChatComposer
          value={draft}
          onChangeText={(text) => setDraft(`agent-thread:${taskId}`, text)}
          onSend={handleSend}
          onStop={handleStop}
          sending={Boolean(pendingTask?.task_id)}
          allowStop={Boolean(taskState.executingTask)}
          allowSubmitWhileRunning
          allowAttachmentOnly
          allowAttachments={canContinue}
          disabled={Boolean(unavailableReason) || !canContinue}
          disabledReason={unavailableReason}
        />
      </KeyboardAvoidingView>
      {unavailableReason ? (
        <View className="absolute inset-x-3 bottom-20 rounded-md bg-destructive/10 px-3 py-2">
          <TextError message={unavailableReason} />
        </View>
      ) : null}
    </View>
  );
}

function TextError({ message }: { message: string }) {
  return (
    <View accessibilityRole="alert">
      <Text className="text-xs text-destructive">{message}</Text>
    </View>
  );
}
