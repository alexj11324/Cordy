import { describe, expect, it } from "vitest";
import type { ChatMessage } from "@orvilo/core/types";
import {
  contentWithInlineCitations,
  validMessageSources,
} from "./message-attribution";

const message = (overrides: Partial<ChatMessage>): ChatMessage => ({
  id: "message-1",
  chat_session_id: "session-1",
  role: "assistant",
  content: "你好 world",
  task_id: "task-1",
  created_at: "2026-09-10T00:00:00Z",
  sources: [{ id: "source-1", url: "https://example.com", title: "Example" }],
  citations: [{ source_id: "source-1", start: 0, end: 6 }],
  ...overrides,
});

describe("mobile message attribution", () => {
  it("places a source marker at the UTF-8 citation boundary", () => {
    expect(contentWithInlineCitations(message({}))).toBe(
      "你好 [1](<https://example.com>) world",
    );
  });

  it("drops unsafe sources and their citations", () => {
    const unsafe = message({
      sources: [{ id: "source-1", url: "javascript:alert(1)" }],
    });
    expect(validMessageSources(unsafe)).toEqual([]);
    expect(contentWithInlineCitations(unsafe)).toBe(unsafe.content);
  });

  it("numbers repeated citations by bibliography source order", () => {
    const repeated = message({
      content: "first second",
      sources: [
        { id: "source-b", url: "https://b.example" },
        { id: "source-a", url: "https://a.example" },
      ],
      citations: [
        { source_id: "source-a", start: 0, end: 5 },
        { source_id: "source-a", start: 6, end: 12 },
      ],
    });
    expect(contentWithInlineCitations(repeated)).toBe(
      "first [2](<https://a.example>) second [2](<https://a.example>)",
    );
  });
});
