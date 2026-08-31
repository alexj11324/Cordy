import type {
  AgentTask,
  ChatMessage,
  ChatPendingTask,
  ChatQueuedTask,
  TimelineEntry,
} from "@patchbay/core/types";
import { stripMentionMarkdown } from "../utils/strip-mention-markdown";

export type IssueConversationActor = {
  actorType: "member" | "agent";
  actorId?: string | null;
};

export type IssueAgentConversation = {
  messages: ChatMessage[];
  messageActors: Record<string, IssueConversationActor>;
  pendingTask: ChatPendingTask | null;
};

const ACTIVE_STATUS_RANK: Partial<Record<AgentTask["status"], number>> = {
  running: 0,
  dispatched: 1,
  waiting_local_directory: 2,
  waiting_capacity: 2,
  queued: 3,
  deferred: 4,
};

function isActiveTask(task: AgentTask): boolean {
  return ACTIVE_STATUS_RANK[task.status] !== undefined;
}

function commentIdsForTask(task: AgentTask): string[] {
  if (task.status !== "queued" && task.delivered_comment_ids !== undefined) {
    return task.delivered_comment_ids;
  }
  return [
    task.trigger_comment_id,
    ...(task.coalesced_comment_ids ?? []),
  ].filter((id): id is string => Boolean(id));
}

function taskResultText(task: AgentTask): string {
  if (typeof task.result === "string") return task.result;
  if (task.result && typeof task.result === "object") {
    const result = task.result as Record<string, unknown>;
    for (const key of ["output", "message", "content"]) {
      if (typeof result[key] === "string") return result[key];
    }
  }
  return task.error ?? "";
}

function elapsedMs(task: AgentTask): number | null {
  if (!task.completed_at) return null;
  const started = new Date(task.started_at ?? task.created_at).getTime();
  const completed = new Date(task.completed_at).getTime();
  if (!Number.isFinite(started) || !Number.isFinite(completed)) return null;
  return Math.max(0, completed - started);
}

/**
 * Projects issue comments + task runs into the shared Chat rendering contract.
 * Provider session ids deliberately stay server-private; the issue/agent pair
 * is the durable conversation identity and every task remains an auditable turn.
 */
export function buildIssueAgentConversation({
  issueId,
  agentId,
  tasks,
  timeline,
  initialRunPrompt,
}: {
  issueId: string;
  agentId: string;
  tasks: AgentTask[];
  timeline: TimelineEntry[];
  initialRunPrompt: string;
}): IssueAgentConversation {
  const conversationId = `issue:${issueId}:agent:${agentId}`;
  const agentTasks = tasks
    .filter((task) => task.agent_id === agentId)
    .toSorted(
      (a, b) =>
        new Date(a.created_at).getTime() - new Date(b.created_at).getTime(),
    );
  const taskIds = new Set(agentTasks.map((task) => task.id));
  const comments = timeline.filter(
    (entry) => entry.type === "comment" && typeof entry.content === "string",
  );
  const commentsById = new Map(
    comments.map((comment) => [comment.id, comment]),
  );
  const assistantCommentsByTask = new Map<string, TimelineEntry[]>();
  for (const comment of comments) {
    if (comment.source_task_id && taskIds.has(comment.source_task_id)) {
      const taskComments =
        assistantCommentsByTask.get(comment.source_task_id) ?? [];
      taskComments.push(comment);
      assistantCommentsByTask.set(comment.source_task_id, taskComments);
    }
  }

  const messages: ChatMessage[] = [];
  const messageActors: Record<string, IssueConversationActor> = {};
  const addedUserComments = new Set<string>();

  for (const task of agentTasks) {
    let addedPrompt = false;
    for (const commentId of commentIdsForTask(task)) {
      const comment = commentsById.get(commentId);
      if (!comment || addedUserComments.has(comment.id)) continue;
      // A target agent's own completion comment is already its assistant turn,
      // even if an older server accidentally included it in a later claim batch.
      if (comment.source_task_id && taskIds.has(comment.source_task_id))
        continue;

      messages.push({
        id: comment.id,
        chat_session_id: conversationId,
        role: "user",
        content: stripMentionMarkdown(comment.content ?? ""),
        task_id: null,
        created_at: comment.created_at,
        attachments: comment.attachments,
      });
      if (
        (comment.actor_type === "member" || comment.actor_type === "agent") &&
        comment.actor_id
      ) {
        messageActors[comment.id] = {
          actorType: comment.actor_type,
          actorId: comment.actor_id,
        };
      }
      addedUserComments.add(comment.id);
      addedPrompt = true;
    }

    if (!addedPrompt && !task.parent_task_id && task.kind !== "message_bus") {
      const content =
        task.handoff_note?.trim() ||
        task.trigger_summary?.trim() ||
        initialRunPrompt;
      const syntheticPromptId = `task-prompt:${task.id}`;
      messages.push({
        id: syntheticPromptId,
        chat_session_id: conversationId,
        role: "user",
        content,
        task_id: null,
        created_at: task.created_at,
      });
      // An assignment or legacy run may not have a source comment. Use its
      // recorded originator when available; the explicit null override keeps
      // unattributed prompts neutral instead of borrowing the current viewer.
      messageActors[syntheticPromptId] = {
        actorType: "member",
        actorId: task.attribution?.originator?.id || null,
      };
    }

    if (isActiveTask(task)) continue;

    const assistantComments = assistantCommentsByTask.get(task.id) ?? [];
    if (assistantComments.length > 0) {
      assistantComments.forEach((comment, index) => {
        const isLastComment = index === assistantComments.length - 1;
        const content = comment.content ?? "";
        messages.push({
          id: comment.id,
          chat_session_id: conversationId,
          role: "assistant",
          content,
          // Attach run reasoning/tool history only once. Earlier issue replies
          // remain ordinary assistant messages rather than duplicating it.
          task_id: isLastComment ? task.id : null,
          created_at: comment.created_at,
          attachments: comment.attachments,
          failure_reason:
            isLastComment && task.status === "failed"
              ? task.failure_reason || "agent_error"
              : null,
          elapsed_ms: isLastComment ? elapsedMs(task) : null,
          message_kind: content.trim() ? "message" : "no_response",
        });
      });
    } else {
      const content = taskResultText(task);
      messages.push({
        id: `task-result:${task.id}`,
        chat_session_id: conversationId,
        role: "assistant",
        content,
        task_id: task.id,
        created_at: task.completed_at ?? task.created_at,
        failure_reason:
          task.status === "failed"
            ? task.failure_reason || "agent_error"
            : null,
        elapsed_ms: elapsedMs(task),
        message_kind: content.trim() ? "message" : "no_response",
      });
    }
  }

  const activeTasks = agentTasks.filter(isActiveTask).toSorted((a, b) => {
    const aIsSideChat = Boolean(a.side_chat_parent_task_id);
    const bIsSideChat = Boolean(b.side_chat_parent_task_id);
    if (aIsSideChat !== bIsSideChat) return aIsSideChat ? -1 : 1;
    if (aIsSideChat && bIsSideChat) {
      return (
        new Date(b.created_at).getTime() - new Date(a.created_at).getTime()
      );
    }
    const rank =
      (ACTIVE_STATUS_RANK[a.status] ?? 99) -
      (ACTIVE_STATUS_RANK[b.status] ?? 99);
    if (rank !== 0) return rank;
    return new Date(a.created_at).getTime() - new Date(b.created_at).getTime();
  });
  const head = activeTasks[0];
  const pendingTask: ChatPendingTask | null = head
    ? {
        task_id: head.id,
        status: head.status,
        created_at: head.created_at,
        supports_queue: true,
        queued_tasks: activeTasks.slice(1).map((task) => ({
          task_id: task.id,
          status: task.status,
          created_at: task.created_at,
          content: task.trigger_summary,
        })),
      }
    : null;

  return {
    messages: messages.toSorted((a, b) => {
      const time =
        new Date(a.created_at).getTime() - new Date(b.created_at).getTime();
      if (time !== 0) return time;
      if (a.role === b.role) return a.id.localeCompare(b.id);
      return a.role === "user" ? -1 : 1;
    }),
    messageActors,
    pendingTask,
  };
}

/**
 * Queue-tray rows for an issue conversation. When the first Wait follow-up
 * is still queued, `buildIssueAgentConversation` promotes it to `pendingTask`
 * and only puts later work in `queued_tasks` — include that head so Edit /
 * Remove / Clear still have a target.
 */
export function queuedIssueFollowUps(
  pendingTask: ChatPendingTask | null,
  tasks: readonly AgentTask[],
): ChatQueuedTask[] {
  if (!pendingTask) return [];
  const rows: ChatQueuedTask[] = [];
  if (
    (pendingTask.status === "queued" || pendingTask.status === "deferred") &&
    pendingTask.task_id &&
    pendingTask.created_at
  ) {
    const head = tasks.find((task) => task.id === pendingTask.task_id);
    rows.push({
      task_id: pendingTask.task_id,
      status: pendingTask.status,
      created_at: pendingTask.created_at,
      content: head?.trigger_summary,
    });
  }
  for (const task of pendingTask.queued_tasks ?? []) {
    if (task.status === "queued" || task.status === "deferred") rows.push(task);
  }
  return rows;
}
