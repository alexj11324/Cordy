import { describe, expect, it } from "vitest";
import broker, { desktopDeepLink, isDesktopPlatform } from "./index";

describe("desktop OAuth handoff contract", () => {
  it("accepts only the explicit desktop platform", () => {
    expect(isDesktopPlatform("desktop")).toBe(true);
    expect(isDesktopPlatform("web")).toBe(false);
    expect(isDesktopPlatform(null)).toBe(false);
  });

  it("puts only an opaque code in the deep link", () => {
    const link = desktopDeepLink("opaque-code");
    expect(link).toBe("patchbay://auth/callback?code=opaque-code");
    expect(link).not.toContain("token");
    expect(link).not.toContain("access_token");
    expect(link).not.toContain("id_token");
  });

  it("keeps liveness and readiness on the broker-owned route", async () => {
    const health = await broker.fetch(
      new Request("https://accounts.aspectlylabs.com/oauth/google/healthz"),
      {},
    );
    expect(health.status).toBe(200);
    expect(await health.json()).toEqual({ service: "auth-broker", status: "ok" });

    const ready = await broker.fetch(
      new Request("https://accounts.aspectlylabs.com/oauth/google/readyz"),
      {},
    );
    expect(ready.status).toBe(503);
    expect(await ready.json()).toEqual({ service: "auth-broker", status: "not_ready" });
  });

  it("keeps Clerk session tasks inside the broker flow", async () => {
    const response = await broker.fetch(
      new Request("https://accounts.aspectlylabs.com/oauth/google?platform=desktop"),
      { CLERK_PUBLISHABLE_KEY: "pk_test_patchbay" },
    );
    const body = await response.text();

    expect(response.status).toBe(200);
    expect(body).toContain('"choose-organization": completeUrl');
    expect(body).toContain('"reset-password": completeUrl');
    expect(body).toContain('"setup-mfa": completeUrl');
  });
});
