// @vitest-environment node
import { describe, expect, it } from "vitest";
import { preferredAppLocaleFromLanguages } from "./os-locale";

describe("preferredAppLocaleFromLanguages", () => {
  it("falls back to English when no language is advertised", () => {
    expect(preferredAppLocaleFromLanguages([])).toBe("en");
  });

  it("keeps English for unrelated locales", () => {
    expect(preferredAppLocaleFromLanguages(["fr-FR"])).toBe("en");
    expect(preferredAppLocaleFromLanguages(["de"])).toBe("en");
  });

  it("maps every Chinese variant to Simplified copy", () => {
    expect(preferredAppLocaleFromLanguages(["zh-CN"])).toBe("zh-Hans");
    expect(preferredAppLocaleFromLanguages(["zh-TW"])).toBe("zh-Hans");
    expect(preferredAppLocaleFromLanguages(["zh-Hant"])).toBe("zh-Hans");
  });

  it("resolves Japanese and Korean from a language prefix", () => {
    expect(preferredAppLocaleFromLanguages(["ja-JP"])).toBe("ja");
    expect(preferredAppLocaleFromLanguages(["ko-KR"])).toBe("ko");
  });
});
