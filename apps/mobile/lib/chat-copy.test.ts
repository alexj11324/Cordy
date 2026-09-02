import { describe, expect, it } from "vitest";
import { createChatCopy, normalizeChatLocale } from "./chat-copy";

describe("mobile chat copy", () => {
  it("normalizes the API language values and unknown values safely", () => {
    expect(normalizeChatLocale("en")).toBe("en");
    expect(normalizeChatLocale("zh")).toBe("zh-Hans");
    expect(normalizeChatLocale("zh-CN")).toBe("zh-Hans");
    expect(normalizeChatLocale("ja-JP")).toBe("ja");
    expect(normalizeChatLocale("ko-KR")).toBe("ko");
    expect(normalizeChatLocale("fr")).toBe("en");
    expect(normalizeChatLocale(null)).toBe("en");
  });

  it("provides the native chat entry copy in all four supported locales", () => {
    const en = createChatCopy("en");
    const zh = createChatCopy("zh-Hans");
    const ja = createChatCopy("ja");
    const ko = createChatCopy("ko");

    expect(en.chat).toBe("Chat");
    expect(zh.chat).toBe("聊天");
    expect(ja.chat).toBe("チャット");
    expect(ko.chat).toBe("채팅");
    expect(new Set([zh.chat, ja.chat, ko.chat]).size).toBe(3);

    for (const copy of [en, zh, ja, ko]) {
      expect(copy.fallbackStarters).toHaveLength(3);
      expect(copy.status.thinking).toBeTruthy();
      expect(copy.failure.labels["agent_error.provider_network"]).toBeTruthy();
      expect(copy.processSteps(1)).toBeTruthy();
      expect(copy.processSteps(2)).toContain("2");
      expect(copy.deleteChatDescription("Example")).toContain("Example");
    }
  });
});
