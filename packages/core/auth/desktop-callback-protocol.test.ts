import { describe, expect, it } from "vitest";

import {
  PRODUCTION_DESKTOP_CALLBACK_PROTOCOL,
  isDesktopCallbackProtocol,
} from "./desktop-callback-protocol";

describe("desktop callback protocols", () => {
  it.each([
    PRODUCTION_DESKTOP_CALLBACK_PROTOCOL,
    "orvilo-canary-5718c47b86bf9ece",
    "orvilo-staging-5718c47b86bf9ece",
  ])("accepts a Orvilo-owned callback protocol: %s", (protocol) => {
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
    "orvilo-staging",
    "orvilo-staging-",
  ])("rejects an unowned callback protocol: %s", (protocol) => {
    expect(isDesktopCallbackProtocol(protocol)).toBe(false);
  });
});
