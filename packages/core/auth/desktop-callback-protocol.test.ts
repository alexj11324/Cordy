import { describe, expect, it } from "vitest";

import {
  DEVELOPMENT_DESKTOP_CALLBACK_PROTOCOL,
  PRODUCTION_DESKTOP_CALLBACK_PROTOCOL,
  isDesktopCallbackProtocol,
} from "./desktop-callback-protocol";

describe("desktop callback protocols", () => {
  it.each([
    PRODUCTION_DESKTOP_CALLBACK_PROTOCOL,
    DEVELOPMENT_DESKTOP_CALLBACK_PROTOCOL,
    "patchbay-canary-login-fix-123",
  ])("accepts a Patchbay-owned callback protocol: %s", (protocol) => {
    expect(isDesktopCallbackProtocol(protocol)).toBe(true);
  });

  it.each([
    "",
    "evil-app",
    "patchbay-preview",
    "patchbay-canary-",
    `patchbay-canary-${"a".repeat(49)}`,
  ])("rejects an unowned callback protocol: %s", (protocol) => {
    expect(isDesktopCallbackProtocol(protocol)).toBe(false);
  });
});
