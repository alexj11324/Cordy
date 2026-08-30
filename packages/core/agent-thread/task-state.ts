import type { AgentTask, ChatPendingTask, ChatQueuedTask } from "../types";

const ACTIVE_TASK_STATUSES = new Set([
  "queued",
  "deferred",
  "dispatched",
  "waiting_local_directory",
  "running",
]);

const EXECUTING_TASK_STATUSES = new Set([
  "dispatched",
  "waiting_local_directory",
  "running",
]);

const QUEUED_TASK_STATUSES = new Set(["queued", "deferred"]);

export function isAgentTaskActive(task: Pick<AgentTask, "status">): boolean {
  return ACTIVE_TASK_STATUSES.has(task.status);
}

export interface AgentThreadTaskState {
  /** The task that owns the provider lane right now, if any. */
  headTask: AgentTask | null;
  /** The task that can be stopped while the provider is executing. */
  executingTask: AgentTask | null;
  /** Deferred/queued children behind the lane head. */
  queuedTasks: ChatQueuedTask[];
  pendingTask: ChatPendingTask | null;
}

/**
 * Derive the provider lane from the complete task chain, not from the
 * envelope's newest task. A continuation child can be queued while its parent
 * is still running; in that state the parent remains the stop/status head and
 * the child is an explicit queue entry.
 */
export function deriveAgentThreadTaskState(
  tasks: readonly AgentTask[],
): AgentThreadTaskState {
  const activeTasks = tasks
    .filter(isAgentTaskActive)
    .slice()
    .sort(
      (left, right) =>
        left.created_at.localeCompare(right.created_at) ||
        left.id.localeCompare(right.id),
    );
  const executingTask =
    activeTasks.find((task) => EXECUTING_TASK_STATUSES.has(task.status)) ??
    null;
  const headTask = executingTask ?? activeTasks[0] ?? null;
  const queuedTasks = activeTasks
    .filter(
      (task) =>
        task.id !== headTask?.id && QUEUED_TASK_STATUSES.has(task.status),
    )
    .map<ChatQueuedTask>((task) => ({
      task_id: task.id,
      status: task.status,
      created_at: task.created_at,
      content: task.trigger_summary?.trim() || task.handoff_note?.trim(),
    }));

  return {
    headTask,
    executingTask,
    queuedTasks,
    pendingTask: headTask
      ? {
          task_id: headTask.id,
          status: headTask.status,
          created_at: headTask.created_at,
          supports_queue: true,
          queued_tasks: queuedTasks,
        }
      : null,
  };
}
