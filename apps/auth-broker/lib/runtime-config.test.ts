import { afterEach, describe, expect, it, vi } from "vitest";
import { readAuthBrokerRuntimeConfig } from "./runtime-config";

const valid = {
  PATCHBAY_API_ORIGIN: "https://api.aspectlylabs.com",
  PATCHBAY_AUTH_BROKER_ORIGIN: "https://accounts.aspectlylabs.com",
  CLERK_PUBLISHABLE_KEY: "pk_test_placeholder",
};

describe("auth broker runtime configuration", () => {
  afterEach(() => vi.unstubAllEnvs());

  it("loads canonical runtime origins without build-time credentials", () => {
    expect(readAuthBrokerRuntimeConfig(valid)).toEqual({
      ok: true,
      config: {
        apiOrigin: "https://api.aspectlylabs.com",
        brokerOrigin: "https://accounts.aspectlylabs.com",
        clerkPublishableKey: "pk_test_placeholder",
      },
    });
  });

  it("fails closed when a required value is missing", () => {
    expect(readAuthBrokerRuntimeConfig({ ...valid, CLERK_PUBLISHABLE_KEY: "" })).toEqual({
      ok: false,
      error: "CLERK_PUBLISHABLE_KEY is required",
    });
  });

  it("rejects insecure or path-bearing production origins", () => {
    vi.stubEnv("NODE_ENV", "production");
    expect(
      readAuthBrokerRuntimeConfig({ ...valid, PATCHBAY_API_ORIGIN: "http://api.example" }),
    ).toMatchObject({ ok: false });
    expect(
      readAuthBrokerRuntimeConfig({
        ...valid,
        PATCHBAY_API_ORIGIN: "https://api.aspectlylabs.com/path",
      }),
    ).toMatchObject({ ok: false });
  });

  it("allows loopback HTTP only for local development", () => {
    vi.stubEnv("NODE_ENV", "development");
    expect(
      readAuthBrokerRuntimeConfig({
        ...valid,
        PATCHBAY_API_ORIGIN: "http://127.0.0.1:8080",
        PATCHBAY_AUTH_BROKER_ORIGIN: "http://localhost:3100",
      }),
    ).toMatchObject({ ok: true });
  });
});
