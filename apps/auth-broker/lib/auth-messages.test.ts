import { describe, expect, it } from "vitest";
import { resolveLocale } from "./auth-messages";

describe("auth broker locale selection", () => {
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
});
