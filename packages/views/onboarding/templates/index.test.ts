import { describe, expect, it } from "vitest";
import { pickContentLang } from "./index";

describe("pickContentLang", () => {
  it("uses the shared locale matcher before selecting persisted content", () => {
    expect(pickContentLang("en-US")).toBe("en");
    expect(pickContentLang("zh-Hant")).toBe("zh");
  });

  it("falls back to English for unsupported or missing languages", () => {
    expect(pickContentLang("fr-FR")).toBe("en");
    expect(pickContentLang("ko-KR")).toBe("en");
    expect(pickContentLang("ja-JP")).toBe("en");
    expect(pickContentLang(null)).toBe("en");
    expect(pickContentLang(undefined)).toBe("en");
  });
});
