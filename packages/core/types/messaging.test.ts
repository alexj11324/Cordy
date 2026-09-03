// @vitest-environment node

import { describe, expect, it } from "vitest";
import { messagingConnectionState } from "./messaging";

describe("messaging connection status", () => {
  const runtime = { state: "healthy", observedAt: "2026-09-03T10:00:00Z", errorCode: null };

  it("does not confuse a saved installation with an observed connection", () => {
    expect(messagingConnectionState({ status: "active" })).toBe("unavailable");
    expect(messagingConnectionState({ status: "active", runtime })).toBe("connected");
    expect(messagingConnectionState({ status: "revoked", runtime })).toBe("disconnected");
  });

  it("keeps quota pauses and experimental transports out of success states", () => {
    expect(messagingConnectionState({ status: "active", runtime: { ...runtime, errorCode: "hosted_quota_paused" } })).toBe("paused");
    expect(messagingConnectionState({ status: "active", runtime, setup: { mode: "managed_token", writable: true, experimental: true } })).toBe("experimental");
  });

  it.each([
    ["starting", "connecting"], ["offline", "disconnected"],
    ["degraded", "degraded"], ["error", "error"], ["future_state", "unavailable"],
  ])("preserves %s without promoting it to connected", (state, expected) => {
    expect(messagingConnectionState({ status: "active", runtime: { ...runtime, state } })).toBe(expected);
  });

  it("requires a valid server observation before showing connected", () => {
    expect(messagingConnectionState({ status: "active", runtime: { ...runtime, observedAt: null } })).toBe("unavailable");
    expect(messagingConnectionState({ status: "active", runtime: { ...runtime, observedAt: "invalid" } })).toBe("unavailable");
  });
});
