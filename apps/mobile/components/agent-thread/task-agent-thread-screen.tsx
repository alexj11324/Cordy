/**
 * The mobile Agent thread surface for a persisted task. It composes the
 * already-shipping ChatMessageList + ChatComposer; this route is the Agent
 * thread surface, not a separate task or event inspection product surface.
 */
import { useCallback, useEffect, useMemo } from "react";
import { Alert, KeyboardAvoidingView, Platform, View } from "react-native";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { ChatMessage, TaskMessagePayload } from "@patchbay/core/types";
import { createSafeId } from "@patchbay/core/utils";
import { api, ApiError } from "@/data/api";
import { chatKeys, taskMessagesOptions } from "@/data/queries/chat";
import { agentThreadOptions } from "@/data/queries/agent-thread";
import { useContinueAgentThread } from "@/data/mutations/agent-thread";
import { useWorkspaceStore } from "@/data/workspace-store";
import { useChatDraftsStore } from "@/data/stores/chat-drafts-store";
import { ChatMessageList } from "@/components/chat/chat-message-list";
import { ChatComposer } from "@/components/chat/chat-composer";
import { Text } from "@/components/ui/text";
import { buildAgentThreadMessages, pendingTaskForAgentThread } from "@/lib/agent-thread-display";
import {
  useAgentThreadCopy,
  type AgentThreadCopy,
} from "@/lib/agent-thread-i18n";

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
    return copy.permission_denied;
  }
  if (error instanceof ApiError && error.status === 409) {
    return copy.unavailable;
  }
  return error instanceof Error ? error.message : copy.could_not_continue;
}

export function TaskAgentThreadScreen({ taskId }: Props) {
  const copy = useAgentThreadCopy();
  const wsId = useWorkspaceStore((state) => state.currentWorkspaceId);
  const queryClient = useQueryClient();
  const threadQuery = useQuery(agentThreadOptions(wsId, taskId));
  const continuation = useContinueAgentThread();
  const draft = useChatDraftsStore((state) => state.drafts[`agent-thread:${taskId}`] ?? "");
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
  const continuationParentTaskId = task?.id ?? taskId;
  const agentName = threadQuery.data?.agent.name?.trim() || copy.agent;
  const messages = useMemo<ChatMessage[]>(
    () =>
      threadTasks.flatMap((threadTask) =>
        buildAgentThreadMessages(threadTask, copy.continue_prompt),
      ),
    [copy.continue_prompt, threadTasks],
  );
  const pendingTask = pendingTaskForAgentThread(task);
  const liveTaskMessages = useQuery(taskMessagesOptions(continuationParentTaskId));

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
    async (content: string, _attachmentIds: string[]) => {
      if (
        !threadQuery.data?.can_continue ||
        threadQuery.data.availability.state !== "available" ||
        !continuationParentTaskId
      ) {
        throw new Error(
          threadQuery.data?.availability.reason ||
            copy.unavailable_fallback,
        );
      }
      await continuation.mutateAsync({
        taskId: continuationParentTaskId,
        request: {
          content: content.trim(),
          idempotency_key: createSafeId(),
        },
      });
      clearDraft(`agent-thread:${taskId}`);
    },
    [
      clearDraft,
      continuation,
      continuationParentTaskId,
      copy.unavailable_fallback,
      taskId,
      threadQuery.data,
    ],
  );

  const handleStop = useCallback(() => {
    if (!continuationParentTaskId) return;
    void api.cancelTaskById(continuationParentTaskId).catch((error) => {
      Alert.alert(copy.unable_to_stop, continuationError(error, copy));
    });
  }, [continuationParentTaskId, copy]);

  const unavailableReason = threadQuery.isError
    ? continuationError(threadQuery.error, copy)
    : threadQuery.data &&
        (!threadQuery.data.can_continue ||
          threadQuery.data.availability.state === "unavailable")
      ? threadQuery.data.availability.reason ||
        copy.unavailable_fallback
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
          agentName={agentName}
          onPickPrompt={(text) => setDraft(`agent-thread:${taskId}`, text)}
          pendingTask={pendingTask}
          liveTaskMessages={liveTaskMessages.data ?? []}
          availability={availability}
        />
        <ChatComposer
          value={draft}
          onChangeText={(text) => setDraft(`agent-thread:${taskId}`, text)}
          onSend={handleSend}
          onStop={handleStop}
          sending={Boolean(pendingTask?.task_id)}
          allowStop={pendingTask?.status !== "queued"}
          allowAttachments={false}
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
