import type { AgentTask, ChatPendingTask, ChatQueuedTask } from "../types";

export function isAgentTaskActive(task: AgentTask): boolean {
  switch (task.status) {
    case "queued":
    case "deferred":
    case "dispatched":
    case "waiting_local_directory":
    case "running":
      return true;
    case "completed":
    case "failed":
    case "cancelled":
      return false;
    default:
      return false;
  }
}

export function deriveAgentThreadTaskState(tasks: AgentTask[]): {
  executingTask?: AgentTask;
  pendingTask: ChatPendingTask | null;
  queuedTasks: ChatQueuedTask[];
} {
  const active = tasks.filter(isAgentTaskActive);
  const executingTask = active.find((task) =>
    task.status === "running" || task.status === "dispatched" || task.status === "waiting_local_directory",
  );
  const head = executingTask ?? active[0];
  return {
    executingTask,
    pendingTask: head
      ? { task_id: head.id, status: head.status, created_at: head.created_at }
      : null,
    queuedTasks: active
      .filter((task) => task.id !== head?.id)
      .map((task) => ({ task_id: task.id, status: task.status, created_at: task.created_at })),
  };
}
