import { describe, expect, it } from "vitest";
import { buildAutomationWebhookUrl, maskAutomationWebhookUrl } from "./webhook";
import type { AutomationTrigger } from "../types";

const baseTrigger: AutomationTrigger = {
  id: "t1",
  automation_id: "a1",
  kind: "webhook",
  enabled: true,
  cron_expression: null,
  timezone: null,
  next_run_at: null,
  webhook_token: "awt_abc",
  webhook_path: "/api/webhooks/automations/awt_abc",
  webhook_url: null,
  label: null,
  last_fired_at: null,
  created_at: "",
  updated_at: "",
};

describe("buildAutomationWebhookUrl", () => {
  it("returns the server-provided webhook_url verbatim when present", () => {
    expect(
      buildAutomationWebhookUrl({
        trigger: { ...baseTrigger, webhook_url: "https://custom.example/api/webhooks/automations/awt_abc" },
      }),
    ).toBe("https://custom.example/api/webhooks/automations/awt_abc");
  });

  it("composes from apiBaseUrl + webhook_path", () => {
    expect(
      buildAutomationWebhookUrl({ trigger: baseTrigger, apiBaseUrl: "https://api.example" }),
    ).toBe("https://api.example/api/webhooks/automations/awt_abc");
  });

  it("strips trailing slash on apiBaseUrl", () => {
    expect(
      buildAutomationWebhookUrl({ trigger: baseTrigger, apiBaseUrl: "https://api.example/" }),
    ).toBe("https://api.example/api/webhooks/automations/awt_abc");
  });

  it("falls back to currentOrigin when apiBaseUrl is empty", () => {
    expect(
      buildAutomationWebhookUrl({
        trigger: baseTrigger,
        apiBaseUrl: "",
        currentOrigin: "https://app.example",
      }),
    ).toBe("https://app.example/api/webhooks/automations/awt_abc");
  });

  it("composes from token when webhook_path is missing", () => {
    expect(
      buildAutomationWebhookUrl({
        trigger: { ...baseTrigger, webhook_path: null },
        apiBaseUrl: "https://api.example",
      }),
    ).toBe("https://api.example/api/webhooks/automations/awt_abc");
  });

  it("returns null for non-webhook trigger", () => {
    expect(
      buildAutomationWebhookUrl({
        trigger: { ...baseTrigger, kind: "schedule", webhook_token: null, webhook_path: null },
      }),
    ).toBeNull();
  });

  it("returns relative path when no base or origin available", () => {
    expect(buildAutomationWebhookUrl({ trigger: baseTrigger })).toBe("/api/webhooks/automations/awt_abc");
  });
});

describe("maskAutomationWebhookUrl", () => {
  it("masks the token segment and keeps the rest readable", () => {
    const masked = maskAutomationWebhookUrl("https://api.example/api/webhooks/automations/awt_abc");
    expect(masked).toBe("https://api.example/api/webhooks/automations/••••••••••••");
    expect(masked).not.toContain("awt_abc");
  });

  it("masks the token on a relative path", () => {
    expect(maskAutomationWebhookUrl("/api/webhooks/automations/awt_abc")).toBe(
      "/api/webhooks/automations/••••••••••••",
    );
  });

  it("uses a fixed-width mask so the token length never leaks", () => {
    const short = maskAutomationWebhookUrl("https://api.example/hooks/a");
    const long = maskAutomationWebhookUrl("https://api.example/hooks/" + "z".repeat(64));
    expect(short).toBe(long);
  });

  it("masks the whole value when there is no separable last segment", () => {
    expect(maskAutomationWebhookUrl("awt_abc")).toBe("••••••••••••");
    expect(maskAutomationWebhookUrl("https://api.example/hooks/")).toBe("••••••••••••");
  });
});
