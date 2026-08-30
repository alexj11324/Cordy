import { describe, expect, it } from "vitest";
import {
  formatAgentThreadCopy,
  normalizeMobileLocale,
} from "./agent-thread-i18n";

describe("mobile Agent thread locale resources", () => {
  it("normalizes supported user language tags and falls back to English", () => {
    expect(normalizeMobileLocale("zh-CN")).toBe("zh-Hans");
    expect(normalizeMobileLocale("ja-JP")).toBe("ja");
    expect(normalizeMobileLocale("ko-KR")).toBe("ko");
    expect(normalizeMobileLocale("fr-FR")).toBe("en");
  });

  it("interpolates accessibility copy without exposing a raw template", () => {
    expect(
      formatAgentThreadCopy("Open Agent thread for {{summary}}", {
        summary: "Task",
      }),
    ).toBe("Open Agent thread for Task");
  });
});
