import type { AgentTask, ChatMessage, ChatPendingTask } from "@patchbay/core/types";

const ACTIVE_TASK_STATUSES = new Set<AgentTask["status"]>([
  "queued",
  "dispatched",
  "running",
  "waiting_local_directory",
]);

export function isAgentTaskActive(task: AgentTask): boolean {
  return ACTIVE_TASK_STATUSES.has(task.status);
}

export function taskResultText(task: AgentTask): string {
  if (typeof task.result === "string") return task.result;
  if (task.result && typeof task.result === "object") {
    const result = task.result as Record<string, unknown>;
    for (const key of ["output", "message", "content"]) {
      if (typeof result[key] === "string") return result[key];
    }
  }
  return task.error ?? "";
}

/**
 * Projects one task into the shared Agent thread message contract. The task
 * event stream is attached to the assistant turn through `task_id`; the
 * ChatMessageList then renders the same tool/status cards used by live Chat.
 */
export function buildTaskAgentThreadMessages(
  task: AgentTask,
  initialPrompt: string,
): ChatMessage[] {
  const conversationId = `task:${task.id}`;
  const messages: ChatMessage[] = [
    {
      id: `task-prompt:${task.id}`,
      chat_session_id: conversationId,
      role: "user",
      content: task.handoff_note?.trim() || task.trigger_summary?.trim() || initialPrompt,
      task_id: null,
      created_at: task.created_at,
    },
  ];

  if (!isAgentTaskActive(task)) {
    const content = taskResultText(task);
    messages.push({
      id: `task-result:${task.id}`,
      chat_session_id: conversationId,
      role: "assistant",
      content,
      task_id: task.id,
      created_at: task.completed_at ?? task.started_at ?? task.created_at,
      failure_reason: task.status === "failed" ? task.failure_reason || "agent_error" : null,
      message_kind: content.trim() ? "message" : "no_response",
    });
  }

  return messages;
}

export function pendingTaskForAgentThread(
  task: AgentTask | undefined,
): ChatPendingTask | null {
  if (!task || !isAgentTaskActive(task)) return null;
  return {
    task_id: task.id,
    status: task.status,
    created_at: task.created_at,
    supports_queue: true,
    queued_tasks: [],
  };
}
