// @vitest-environment node
import { describe, expect, it } from "vitest";
import {
  DEFAULT_RUNTIME_CONFIG,
  normalizePackagedRuntimeConfig,
  type RuntimeConfig,
} from "./runtime-config";

function managedConfig(accountsUrl: string): RuntimeConfig {
  return {
    ...DEFAULT_RUNTIME_CONFIG,
    accountsUrl,
  };
}

describe("packaged runtime config loopback repair", () => {
  it.each([
    "http://localhost:3000",
    "http://desktop.localhost:3000",
    "http://127.0.0.1:3000",
    "http://127.0.0.2:3000",
    "https://127.255.255.254",
    "http://[::1]:3000",
  ])("repairs managed accounts loopback origin %s", (accountsUrl) => {
    expect(normalizePackagedRuntimeConfig(managedConfig(accountsUrl))).toEqual(
      DEFAULT_RUNTIME_CONFIG,
    );
  });

  it("does not confuse a numeric-looking DNS name with IPv4 loopback", () => {
    const config = managedConfig("https://127.example.com");
    expect(normalizePackagedRuntimeConfig(config)).toEqual(config);
  });

  it("preserves an explicit self-hosted API even when its accounts broker is local", () => {
    const config: RuntimeConfig = {
      schemaVersion: 1,
      apiUrl: "https://api.example.com",
      wsUrl: "wss://api.example.com/ws",
      appUrl: "https://app.example.com",
      accountsUrl: "http://127.0.0.2:3000",
    };

    expect(normalizePackagedRuntimeConfig(config)).toEqual(config);
  });
});
