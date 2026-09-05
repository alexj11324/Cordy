// @vitest-environment node
import { describe, expect, it } from "vitest";
import {
  isDesktopDeepLink,
  resolveDesktopCallbackProtocol,
} from "./callback-protocol";

describe("desktop callback protocol", () => {
  it("keeps packaged Desktop on patchbay://", () => {
    expect(
      resolveDesktopCallbackProtocol({
        packaged: true,
        developmentProtocol: "patchbay-canary-5718c47b86bf9ece",
      }),
    ).toBe("patchbay");
  });

  it("isolates Canary and linked worktrees from production and each other", () => {
    expect(
      resolveDesktopCallbackProtocol({
        packaged: false,
        developmentProtocol: "patchbay-canary-5718c47b86bf9ece",
      }),
    ).toBe("patchbay-canary-5718c47b86bf9ece");
  });

  it("rejects a missing or shared development protocol", () => {
    expect(() =>
      resolveDesktopCallbackProtocol({ packaged: false }),
    ).toThrow("development callback protocol");
    expect(() =>
      resolveDesktopCallbackProtocol({
        packaged: false,
        developmentProtocol: "patchbay",
      }),
    ).toThrow("development callback protocol");
  });

  it("accepts only this app's exact deep-link protocol", () => {
    expect(
      isDesktopDeepLink(
        "patchbay-canary-5718c47b86bf9ece://auth/callback?code=a&state=b",
        "patchbay-canary-5718c47b86bf9ece",
      ),
    ).toBe(true);
    expect(
      isDesktopDeepLink(
        "patchbay://auth/callback?code=a&state=b",
        "patchbay-canary-5718c47b86bf9ece",
      ),
    ).toBe(false);
    expect(
      isDesktopDeepLink(
        "patchbay-canary-30a2dba77c3584f0://auth/callback?code=a&state=b",
        "patchbay-canary-5718c47b86bf9ece",
      ),
    ).toBe(false);
  });

  it("keeps invite and auth links on the same owned protocol", () => {
    expect(
      isDesktopDeepLink(
        "patchbay-canary-5718c47b86bf9ece://invite/123",
        "patchbay-canary-5718c47b86bf9ece",
      ),
    ).toBe(true);
  });
});
