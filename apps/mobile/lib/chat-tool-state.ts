import type { TaskMessagePayload, TaskMessageState } from "@orvilo/core/types";

/**
 * AI Elements represents a tool call's lifecycle with one state value. The
 * mobile transcript receives the lifecycle as separate task-message rows, so
 * keep the server-provided state when it exists and use the protocol defaults
 * only for legacy rows that predate the state field.
 */
export function taskMessageState(
  item: Pick<TaskMessagePayload, "type" | "state">,
): TaskMessageState {
  if (item.state) return item.state;
  return item.type === "tool_result" ? "output-available" : "input-available";
}

export function foldToolLifecycles(
  items: readonly TaskMessagePayload[],
): TaskMessagePayload[] {
  const folded: TaskMessagePayload[] = [];
  for (const item of [...items].sort((a, b) => a.seq - b.seq)) {
    if (
      item.call_id &&
      (item.type === "tool_use" || item.type === "tool_result")
    ) {
      const existingIndex = folded.findIndex(
        (candidate) =>
          candidate.call_id === item.call_id &&
          (candidate.type === "tool_use" || candidate.type === "tool_result"),
      );
      if (existingIndex >= 0) {
        const existing = folded[existingIndex]!;
        folded[existingIndex] = {
          ...existing,
          type:
            existing.type === "tool_use" || item.type === "tool_use"
              ? "tool_use"
              : "tool_result",
          state: item.state ?? existing.state,
          tool: item.tool ?? existing.tool,
          input: item.input ?? existing.input,
          output: item.output ?? existing.output,
          created_at: item.created_at ?? existing.created_at,
        };
        continue;
      }
    }
    folded.push(item);
  }
  return folded;
}
