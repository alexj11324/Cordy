// @vitest-environment node
import { describe, expect, it } from "vitest";
import {
  isDesktopDeepLink,
  parseDesktopPreviewIdentity,
  resolveDesktopCallbackProtocol,
} from "./callback-protocol";

describe("desktop callback protocol", () => {
  it("keeps packaged Desktop on orvilo://", () => {
    expect(
      resolveDesktopCallbackProtocol({
        channel: "production",
        developmentProtocol: "orvilo-canary-5718c47b86bf9ece",
      }),
    ).toBe("orvilo");
  });

  it("uses build-owned identity for packaged previews without changing production", () => {
    const identity = parseDesktopPreviewIdentity({
      bundleId: "ai.orvilo.desktop.canary.5718c47b86bf9ece",
      callbackProtocol: "orvilo-canary-5718c47b86bf9ece",
      name: "Orvilo Canary first", dataName: "Orvilo Canary first",
    });
    expect(resolveDesktopCallbackProtocol({
      channel: "production",
      previewIdentity: identity,
    }))
      .toBe("orvilo-canary-5718c47b86bf9ece");
    expect(parseDesktopPreviewIdentity(undefined)).toBeNull();
    expect(() => parseDesktopPreviewIdentity({ ...identity, callbackProtocol: "orvilo" })).toThrow();
    expect(() => parseDesktopPreviewIdentity({ ...identity, dataName: "../../other" })).toThrow();
  });

  it("isolates Canary and linked worktrees from production and each other", () => {
    expect(
      resolveDesktopCallbackProtocol({
        channel: "development",
        developmentProtocol: "orvilo-canary-5718c47b86bf9ece",
      }),
    ).toBe("orvilo-canary-5718c47b86bf9ece");
  });

  it("isolates Desktop staging from Canary and production callbacks", () => {
    expect(
      resolveDesktopCallbackProtocol({
        channel: "staging",
        developmentProtocol: "orvilo-staging-5718c47b86bf9ece",
      }),
    ).toBe("orvilo-staging-5718c47b86bf9ece");
    expect(() =>
      resolveDesktopCallbackProtocol({
        channel: "staging",
        developmentProtocol: "orvilo",
      }),
    ).toThrow("staging callback protocol");
    expect(() =>
      resolveDesktopCallbackProtocol({
        channel: "staging",
        developmentProtocol: "orvilo-canary-5718c47b86bf9ece",
      }),
    ).toThrow("staging callback protocol");
    expect(() =>
      resolveDesktopCallbackProtocol({
        channel: "staging",
        developmentProtocol: "orvilo-staging",
      }),
    ).toThrow("staging callback protocol");
  });

  it("rejects a missing or shared development protocol", () => {
    expect(() =>
      resolveDesktopCallbackProtocol({ channel: "development" }),
    ).toThrow("development callback protocol");
    expect(() =>
      resolveDesktopCallbackProtocol({
        channel: "development",
        developmentProtocol: "orvilo",
      }),
    ).toThrow("development callback protocol");
  });

  it("accepts only this app's exact deep-link protocol", () => {
    expect(
      isDesktopDeepLink(
        "orvilo-canary-5718c47b86bf9ece://auth/callback?code=a&state=b",
        "orvilo-canary-5718c47b86bf9ece",
      ),
    ).toBe(true);
    expect(
      isDesktopDeepLink(
        "orvilo://auth/callback?code=a&state=b",
        "orvilo-canary-5718c47b86bf9ece",
      ),
    ).toBe(false);
    expect(
      isDesktopDeepLink(
        "orvilo-canary-30a2dba77c3584f0://auth/callback?code=a&state=b",
        "orvilo-canary-5718c47b86bf9ece",
      ),
    ).toBe(false);
  });

  it("keeps invite and auth links on the same owned protocol", () => {
    expect(
      isDesktopDeepLink(
        "orvilo-canary-5718c47b86bf9ece://invite/123",
        "orvilo-canary-5718c47b86bf9ece",
      ),
    ).toBe(true);
  });
});
