// @vitest-environment node
import { describe, it, expect } from "vitest";
import type { ChatMessage } from "@patchbay/core/types";
import type { ChatTimelineItem } from "@patchbay/core/chat";
import { extractCopyText } from "./copy-text";

const text = (seq: number, content: string): ChatTimelineItem => ({
  seq,
  type: "text",
  content,
});

const thinking = (seq: number, content = "..."): ChatTimelineItem => ({
  seq,
  type: "thinking",
  content,
});

const tool = (seq: number, name = "Read"): ChatTimelineItem => ({
  seq,
  type: "tool_use",
  tool: name,
  input: { path: "/x" },
});

const message = (content: string): ChatMessage => ({
  id: "m1",
  chat_session_id: "s1",
  role: "assistant",
  content,
  task_id: "t1",
  created_at: "2026-05-06T00:00:00Z",
});

describe("extractCopyText", () => {
  it("falls back to message.content when timeline is empty (legacy)", () => {
    expect(extractCopyText(message("legacy body"), [])).toBe("legacy body");
  });

  it("returns concatenated text segments for an all-text timeline", () => {
    expect(
      extractCopyText(message(""), [text(1, "hello"), text(2, "world")]),
    ).toBe("hello\n\nworld");
  });

  it("includes every visible text segment and omits thinking and tools", () => {
    expect(
      extractCopyText(message(""), [
        thinking(1),
        tool(2),
        text(3, "intermediate"),
        tool(4),
        text(5, "final answer"),
      ]),
    ).toBe("intermediate\n\nfinal answer");
  });

  it("includes preface, intermediate text, and final text", () => {
    expect(
      extractCopyText(message(""), [
        text(1, "preface"),
        tool(2),
        text(3, "middle"),
        tool(4),
        text(5, "final"),
      ]),
    ).toBe("preface\n\nmiddle\n\nfinal");
  });

  it("falls back to message.content when timeline has no text items", () => {
    expect(
      extractCopyText(message("fallback body"), [thinking(1), tool(2)]),
    ).toBe("fallback body");
  });

  it("joins multiple trailing text segments with blank-line separators", () => {
    expect(
      extractCopyText(message(""), [
        tool(1),
        text(2, "para 1"),
        text(3, "para 2"),
      ]),
    ).toBe("para 1\n\npara 2");
  });
});
