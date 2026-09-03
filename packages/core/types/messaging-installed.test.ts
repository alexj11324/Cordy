// @vitest-environment node
import { expect, it } from "vitest";
import { messagingConnectionState } from "./messaging";

it("uses installed as the installation state without accepting the old active value", () => {
  const runtime = {
    state: "healthy",
    observedAt: "2026-09-03T12:00:00Z",
    errorCode: null,
  };
  expect(messagingConnectionState({ status: "installed", runtime })).toBe(
    "connected",
  );
  expect(messagingConnectionState({ status: "active", runtime })).toBe(
    "unavailable",
  );
});
