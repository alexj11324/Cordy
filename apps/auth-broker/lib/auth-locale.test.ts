import { describe, expect, it } from "vitest";
import { resolveAuthLocale } from "./auth-locale";

describe("Accounts locale negotiation", () => {
  it.each([
    ["zh-CN,zh;q=0.9", "zh-Hans", "zh-CN"],
    ["ja-JP", "ja", "ja-JP"],
    ["ko-KR", "ko", "ko-KR"],
    ["en-US", "en", "en"],
  ])("maps %s to the translated locale and html lang", (header, locale, htmlLang) => {
    expect(resolveAuthLocale(header)).toEqual({ locale, htmlLang });
  });

  it("falls back to English for missing or unsupported languages", () => {
    expect(resolveAuthLocale(undefined)).toEqual({ locale: "en", htmlLang: "en" });
    expect(resolveAuthLocale("fr-FR")).toEqual({ locale: "en", htmlLang: "en" });
  });
});
