import { describe, expect, it } from "vitest";
import { createChatCopy, normalizeChatLocale } from "./chat-copy";

describe("mobile chat copy", () => {
  it("normalizes the API language values and unknown values safely", () => {
    expect(normalizeChatLocale("en")).toBe("en");
    expect(normalizeChatLocale("zh")).toBe("zh-Hans");
    expect(normalizeChatLocale("zh-CN")).toBe("zh-Hans");
    expect(normalizeChatLocale("ja-JP")).toBe("en");
    expect(normalizeChatLocale("ko-KR")).toBe("en");
    expect(normalizeChatLocale("fr")).toBe("en");
    expect(normalizeChatLocale(null)).toBe("en");
  });

  it("provides the native chat entry copy in every supported locale", () => {
    const en = createChatCopy("en");
    const zh = createChatCopy("zh-Hans");

    expect(en.chat).toBe("Chat");
    expect(zh.chat).toBe("聊天");
    expect(new Set([en.chat, zh.chat]).size).toBe(2);

    for (const copy of [en, zh]) {
      expect(copy.fallbackStarters).toHaveLength(3);
      expect(copy.status.thinking).toBeTruthy();
      expect(copy.failure.labels["agent_error.provider_network"]).toBeTruthy();
      expect(copy.processSteps(1)).toBeTruthy();
      expect(copy.processSteps(2)).toContain("2");
      expect(copy.queueTitle(2)).toContain("2");
      expect(copy.queueFallback).toBeTruthy();
      expect(copy.toolState["output-available"]).toBeTruthy();
      expect(copy.toolState["output-error"]).toBeTruthy();
      expect(copy.deleteChatDescription("Example")).toContain("Example");
      expect(copy.openChatWith("Example")).toContain("Example");
    }
  });
});
