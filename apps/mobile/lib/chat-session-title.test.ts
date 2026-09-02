import { describe, expect, it } from "vitest";
import { chatSessionDisplayTitle } from "./chat-session-title";

describe("chatSessionDisplayTitle", () => {
  it("uses New chat for an explicitly empty channel-created Chat", () => {
    expect(chatSessionDisplayTitle("")).toBe("New chat");
    expect(chatSessionDisplayTitle(null)).toBe("New chat");
    expect(chatSessionDisplayTitle(undefined)).toBe("New chat");
  });

  it("preserves a stored or manually renamed title", () => {
    expect(chatSessionDisplayTitle("Investigate deploy")).toBe(
      "Investigate deploy",
    );
  });

  it("accepts a localized fallback for a blank session title", () => {
    expect(chatSessionDisplayTitle(null, "新对话")).toBe("新对话");
  });
});
