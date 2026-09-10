import type {
  AgentTask,
  ChatMessage,
  MessageCitation,
  MessageSource,
  TaskMessagePayload,
} from "@orvilo/core/types";
import { isAgentTaskActive } from "@orvilo/core/agent-thread";

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

export function buildTaskAgentThreadMessages(
  task: AgentTask,
  initialPrompt: string,
  events: TaskMessagePayload[] = [],
): ChatMessage[] {
  const conversationId = `task:${task.id}`;
  const messages: ChatMessage[] = [{
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
  }];

  if (!isAgentTaskActive(task)) {
    const content = taskResultText(task);
    const attribution = taskMessageAttribution(events, content);
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
      ...(task.usage ? { usage: task.usage } : {}),
      ...(attribution ?? {}),
    });
  }
  return messages;
}

/**
 * Promote the task transcript's provider-authored attribution onto the
 * synthesized assistant message shown in an Agent thread. Event citations are
 * local to each text event, so concatenate the UTF-8 byte ranges while
 * preserving source order and remapping conflicting source IDs.
 */
function taskMessageAttribution(
  events: TaskMessagePayload[],
  content: string,
): Pick<ChatMessage, "sources" | "citations"> | undefined {
  if (!content || events.length === 0) return undefined;
  const textEvents = events.filter(
    (event) => event.type === "text" && typeof event.content === "string",
  );
  if (textEvents.length === 0) return undefined;

  const sourceById = new Map<string, MessageSource>();
  const sources: MessageSource[] = [];
  const citations: MessageCitation[] = [];
  let concatenated = "";

  for (const event of textEvents) {
    const base = new TextEncoder().encode(concatenated).length;
    concatenated += event.content;
    const remappedIds = new Map<string, string>();
    for (const source of event.sources ?? []) {
      const originalId = source.id;
      let id = originalId;
      const existing = sourceById.get(id);
      if (existing && (existing.url !== source.url || existing.title !== source.title)) {
        for (let suffix = 2; ; suffix += 1) {
          const candidate = `${originalId}#${suffix}`;
          if (!sourceById.has(candidate)) {
            id = candidate;
            break;
          }
        }
      }
      remappedIds.set(originalId, id);
      if (!sourceById.has(id)) {
        const normalized = { ...source, id };
        sourceById.set(id, normalized);
        sources.push(normalized);
      }
    }
    for (const citation of event.citations ?? []) {
      citations.push({
        ...citation,
        source_id: remappedIds.get(citation.source_id) ?? citation.source_id,
        start: citation.start + base,
        end: citation.end + base,
      });
    }
  }

  // Never attach ranges to a different string. Older task APIs can return a
  // summarized result without the complete text event stream.
  if (concatenated !== content || citations.length === 0) return undefined;
  return { sources, citations };
}
