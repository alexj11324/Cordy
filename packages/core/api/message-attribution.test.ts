import { describe, expect, it } from "vitest";
import { ChatMessageSchema } from "./schemas";
import { parseWSFrame } from "./ws-schema";

const source = {
  id: "docs",
  url: "https://example.test/docs",
  title: "Docs",
};
const citation = { source_id: "docs", start: 0, end: 4 };

describe("structured message attribution", () => {
  it("preserves explicit chat sources and never derives them from Markdown", () => {
    const plain = ChatMessageSchema.parse({
      id: "m1",
      chat_session_id: "s1",
      role: "assistant",
      content: "[Docs](https://example.test/docs)",
    });
    expect(plain.sources).toEqual([]);
    expect(plain.citations).toEqual([]);

    const attributed = ChatMessageSchema.parse({
      id: "m2",
      chat_session_id: "s1",
      role: "assistant",
      content: "done",
      sources: [source],
      citations: [citation],
    });
    expect(attributed.sources).toEqual([source]);
    expect(attributed.citations).toEqual([citation]);
  });

  it("keeps attribution on task-message and chat-done websocket frames", () => {
    for (const [type, payload] of [
      ["task:message", { task_id: "t1", seq: 1, type: "text", content: "done", sources: [source], citations: [citation] }],
      ["chat:done", { chat_session_id: "s1", task_id: "t1", message_id: "m1", content: "done", sources: [source], citations: [citation] }],
    ] as const) {
      const parsed = parseWSFrame({ type, payload });
      expect(parsed.kind).toBe("event");
      if (parsed.kind === "event") {
        expect((parsed.message.payload as { sources?: unknown }).sources).toEqual([source]);
        expect((parsed.message.payload as { citations?: unknown }).citations).toEqual([citation]);
      }
    }
  });
});
