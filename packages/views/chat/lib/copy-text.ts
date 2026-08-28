import type { ChatMessage } from "@cordy/core/types";
import type { ChatTimelineItem } from "@cordy/core/chat";

/**
 * Markdown source the Copy action puts on the clipboard. By design this is
 * the user-visible answer only: every text segment in the document flow is
 * included, while reasoning, tool diagnostics, and errors stay out. Falls
 * back to `message.content` for legacy or all-non-text timelines so Copy never
 * produces an empty string.
 */
export function extractCopyText(
  message: ChatMessage,
  timeline: ChatTimelineItem[],
): string {
  if (timeline.length === 0) return message.content ?? "";
  const pieces = timeline
    .filter((item) => item.type === "text")
    .map((i) => i.content ?? "")
    .filter((s) => s.length > 0);
  if (pieces.length === 0) return message.content ?? "";
  return pieces.join("\n\n");
}
