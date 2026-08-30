import type {
  AgentTask,
  ChatMessage,
  ChatPendingTask,
} from "@patchbay/core/types";
import {
  deriveAgentThreadTaskState,
  isAgentTaskActive,
} from "@patchbay/core/agent-thread";

export { isAgentTaskActive };

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
      content:
        task.agent_thread_message?.trim() ||
        task.handoff_note?.trim() ||
        task.trigger_summary?.trim() ||
        initialPrompt,
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
      failure_reason:
        task.status === "failed" ? task.failure_reason || "agent_error" : null,
      message_kind: content.trim() ? "message" : "no_response",
    });
  }

  return messages;
}

export function pendingTaskForAgentThread(
  task: AgentTask | undefined,
): ChatPendingTask | null {
  return task ? deriveAgentThreadTaskState([task]).pendingTask : null;
}
