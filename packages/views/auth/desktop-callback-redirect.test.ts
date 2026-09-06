// Canonical coverage for the desktop OS-callback URL. The web callback page
// is wiring only; do not re-run this matrix through a Next page mount.
import { afterEach, describe, expect, it, vi } from "vitest";
import { redirectToDesktopApp } from "./login-page";

describe("redirectToDesktopApp", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function captureHref(): ReturnType<typeof vi.fn> {
    const hrefSetter = vi.fn();
    vi.stubGlobal("location", {
      assign: vi.fn(),
      replace: vi.fn(),
      set href(value: string) {
        hrefSetter(value);
      },
    });
    return hrefSetter;
  }

  it("opens the stored staging scheme instead of production patchbay://", () => {
    const hrefSetter = captureHref();
    redirectToDesktopApp(
      "staging-code",
      "desktop-state",
      "patchbay-staging-5718c47b86bf9ece",
    );
    expect(hrefSetter).toHaveBeenCalledWith(
      "patchbay-staging-5718c47b86bf9ece://auth/callback?code=staging-code&state=desktop-state",
    );
  });

  it("keeps packaged production on patchbay://", () => {
    const hrefSetter = captureHref();
    redirectToDesktopApp("one-time-code", "desktop-state");
    expect(hrefSetter).toHaveBeenCalledWith(
      "patchbay://auth/callback?code=one-time-code&state=desktop-state",
    );
  });

  it("rejects an unowned callback scheme", () => {
    captureHref();
    expect(() =>
      redirectToDesktopApp("code", "state", "evil-app"),
    ).toThrow("Invalid desktop callback protocol");
  });
});
