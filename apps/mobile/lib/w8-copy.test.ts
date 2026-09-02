import { describe, expect, it } from "vitest";
import {
  W8_COPY_LOCALES,
  getW8Copy,
  normalizeW8Locale,
} from "./w8-copy";

function leafKeys(value: Record<string, unknown>, prefix = ""): string[] {
  return Object.entries(value).flatMap(([key, child]) => {
    const path = prefix ? `${prefix}.${key}` : key;
    return typeof child === "object" && child !== null
      ? leafKeys(child as Record<string, unknown>, path)
      : [path];
  });
}

describe("W8 locale copy", () => {
  it("normalizes API language variants to the supported four locales", () => {
    expect(normalizeW8Locale(null)).toBe("en");
    expect(normalizeW8Locale("zh-CN")).toBe("zh-Hans");
    expect(normalizeW8Locale("zh-Hant")).toBe("zh-Hans");
    expect(normalizeW8Locale("ja-JP")).toBe("en");
    expect(normalizeW8Locale("ko")).toBe("ko");
  });

  it("keeps the channel, WeCom, and bind copy contract complete in all locales", () => {
    const expected = leafKeys(getW8Copy("en"));
    for (const locale of W8_COPY_LOCALES) {
      const copy = getW8Copy(locale);
      expect(leafKeys(copy)).toEqual(expected);
      for (const key of leafKeys(copy)) {
        const value = key.split(".").reduce<unknown>(
          (current, part) =>
            current && typeof current === "object"
              ? (current as Record<string, unknown>)[part]
              : undefined,
          copy,
        );
        expect(value, `${locale}:${key}`).toEqual(expect.any(String));
        expect(String(value).trim(), `${locale}:${key}`).not.toBe("");
      }
    }
  });
});
