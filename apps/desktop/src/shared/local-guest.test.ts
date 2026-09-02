import { describe, expect, it } from "vitest";
import {
  normalizeGuestDisplayName,
  parseLocalGuestSession,
  parseLocalRuntimeProbe,
} from "./local-guest";

describe("local Guest validation", () => {
  it("normalizes a valid display name", () => {
    expect(normalizeGuestDisplayName("  Alice  ")).toBe("Alice");
    expect(normalizeGuestDisplayName("e\u0301")).toBe("é");
  });

  it("rejects empty, oversized, and control-character names", () => {
    expect(normalizeGuestDisplayName("   ")).toBeNull();
    expect(normalizeGuestDisplayName("a".repeat(65))).toBeNull();
    expect(normalizeGuestDisplayName("Alice\n")).toBeNull();
    expect(normalizeGuestDisplayName("Alice\u2028")).toBeNull();
  });

  it("fails closed for non-canonical persisted session data", () => {
    expect(parseLocalGuestSession({ displayName: "Alice" })).toEqual({
      displayName: "Alice",
    });
    expect(parseLocalGuestSession({ displayName: " Alice" })).toBeNull();
    expect(
      parseLocalGuestSession({ displayName: "Alice", token: "secret" }),
    ).toBeNull();
    expect(parseLocalGuestSession(null)).toBeNull();
  });

  it("accepts only a self-consistent local runtime inventory", () => {
    expect(
      parseLocalRuntimeProbe({
        probe_result: "success",
        runtime_count: 2,
        provider_summary: { claude: 1, codex: 1 },
      }),
    ).toEqual({
      probeResult: "success",
      runtimeCount: 2,
      providerSummary: { claude: 1, codex: 1 },
      onlineCount: 0,
      offlineCount: 2,
    });
    expect(
      parseLocalRuntimeProbe({
        probe_result: "success",
        runtime_count: 2,
        provider_summary: { claude: 1 },
      }),
    ).toEqual({ probeResult: "error" });
    expect(
      parseLocalRuntimeProbe({
        probe_result: "success",
        runtime_count: 1,
        provider_summary: { claude: -1 },
      }),
    ).toEqual({ probeResult: "error" });
  });
});
