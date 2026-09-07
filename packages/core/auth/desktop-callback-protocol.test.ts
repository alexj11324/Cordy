import { describe, expect, it } from "vitest";

import {
  PRODUCTION_DESKTOP_CALLBACK_PROTOCOL,
  isDesktopCallbackProtocol,
} from "./desktop-callback-protocol";

describe("desktop callback protocols", () => {
  it.each([
    PRODUCTION_DESKTOP_CALLBACK_PROTOCOL,
    "orvilo-canary-5718c47b86bf9ece",
  ])("accepts an Orvilo-owned callback protocol: %s", (protocol) => {
    expect(isDesktopCallbackProtocol(protocol)).toBe(true);
  });

  it.each([
    "",
    "evil-app",
    "orvilo-preview",
    "orvilo-canary",
    "orvilo-canary-",
    "orvilo-canary-01zp-25",
    "orvilo-canary-login-fix-123",
    `orvilo-canary-${"a".repeat(49)}`,
  ])("rejects an unowned callback protocol: %s", (protocol) => {
    expect(isDesktopCallbackProtocol(protocol)).toBe(false);
  });
});
