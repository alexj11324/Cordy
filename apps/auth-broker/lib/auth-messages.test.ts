import { renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { resolveLocale, useAuthMessages } from "./auth-messages";

describe("auth broker locale selection", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    document.documentElement.lang = "en";
  });

  it.each([
    [["en-US"], "en"],
    [["zh-TW"], "zh-Hans"],
    [["ja-JP"], "ja"],
    [["ko-KR"], "ko"],
    [["fr-FR", "zh-CN"], "zh-Hans"],
    [["fr-FR"], "en"],
  ] as const)("maps %j to %s", (languages, expected) => {
    expect(resolveLocale(languages)).toBe(expected);
  });

  it("keeps the document language synchronized with localized messages", async () => {
    vi.spyOn(window.navigator, "languages", "get").mockReturnValue(["ja-JP"]);

    const { result } = renderHook(() => useAuthMessages());

    await waitFor(() => expect(result.current.locale).toBe("ja"));
    expect(document.documentElement.lang).toBe("ja");
  });
});
