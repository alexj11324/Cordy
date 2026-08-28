import type {
  AgentTask,
  ChatMessage,
  ChatPendingTask,
  TimelineEntry,
} from "@patchbay/core/types";
import { stripMentionMarkdown } from "../utils/strip-mention-markdown";

export type IssueConversationActor = {
  actorType: "member" | "agent";
  actorId: string;
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
  queued: 3,
};

function isActiveTask(task: AgentTask): boolean {
  return ACTIVE_STATUS_RANK[task.status] !== undefined;
}

function commentIdsForTask(task: AgentTask): string[] {
  if (task.status !== "queued" && task.delivered_comment_ids !== undefined) {
    return task.delivered_comment_ids;
  }
  return [task.trigger_comment_id, ...(task.coalesced_comment_ids ?? [])].filter(
    (id): id is string => Boolean(id),
  );
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
  const commentsById = new Map(comments.map((comment) => [comment.id, comment]));
  const assistantCommentByTask = new Map<string, TimelineEntry>();
  for (const comment of comments) {
    if (comment.source_task_id && taskIds.has(comment.source_task_id)) {
      assistantCommentByTask.set(comment.source_task_id, comment);
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
      if (comment.source_task_id && taskIds.has(comment.source_task_id)) continue;

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
      messages.push({
        id: `task-prompt:${task.id}`,
        chat_session_id: conversationId,
        role: "user",
        content,
        task_id: null,
        created_at: task.created_at,
      });
    }

    if (isActiveTask(task)) continue;

    const comment = assistantCommentByTask.get(task.id);
    const content = comment?.content ?? taskResultText(task);
    messages.push({
      id: comment?.id ?? `task-result:${task.id}`,
      chat_session_id: conversationId,
      role: "assistant",
      content,
      task_id: task.id,
      created_at: comment?.created_at ?? task.completed_at ?? task.created_at,
      attachments: comment?.attachments,
      failure_reason: task.status === "failed" ? task.failure_reason || "agent_error" : null,
      elapsed_ms: elapsedMs(task),
      message_kind: content.trim() ? "message" : "no_response",
    });
  }

  const activeTasks = agentTasks
    .filter(isActiveTask)
    .toSorted((a, b) => {
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
        new Date(a.created_at).getTime() -
        new Date(b.created_at).getTime();
      if (time !== 0) return time;
      if (a.role === b.role) return a.id.localeCompare(b.id);
      return a.role === "user" ? -1 : 1;
    }),
    messageActors,
    pendingTask,
  };
}
