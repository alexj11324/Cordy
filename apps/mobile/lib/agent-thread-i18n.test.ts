import { describe, expect, it } from "vitest";
import {
  agentThreadAvailabilityMessage,
  formatAgentThreadCopy,
  normalizeMobileLocale,
} from "./agent-thread-i18n";
import en from "@/locales/en/agent-thread";

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

  it("localizes known availability codes and only falls back to server copy for unknown codes", () => {
    expect(
      agentThreadAvailabilityMessage(
        en,
        "provider_session_retired",
        "English server text",
      ),
    ).toBe(en.reason_provider_session_retired);
    expect(
      agentThreadAvailabilityMessage(
        en,
        "provider_future",
        "Server explanation",
      ),
    ).toBe("Server explanation");
  });
});
