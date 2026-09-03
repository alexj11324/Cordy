import { beforeEach, describe, expect, it } from "vitest";
import { useWindowOverlayStore } from "./window-overlay-store";

beforeEach(() => {
  useWindowOverlayStore.getState().close();
});

describe("validateSettingsWorkspace", () => {
  it("closes Settings when its workspace leaves the authoritative list", () => {
    const store = useWindowOverlayStore.getState();
    store.open({ type: "settings", path: "/acme/settings?tab=members" });

    store.validateSettingsWorkspace(new Set(["other"]));

    expect(useWindowOverlayStore.getState().overlay).toBeNull();
  });

  it("keeps Settings open while its workspace remains valid", () => {
    const store = useWindowOverlayStore.getState();
    store.open({ type: "settings", path: "/acme/settings" });

    store.validateSettingsWorkspace(new Set(["acme"]));

    expect(useWindowOverlayStore.getState().overlay).toEqual({
      type: "settings",
      path: "/acme/settings",
    });
  });

  it("does not affect pre-workspace overlays", () => {
    const store = useWindowOverlayStore.getState();
    store.open({ type: "onboarding" });

    store.validateSettingsWorkspace(new Set());

    expect(useWindowOverlayStore.getState().overlay).toEqual({
      type: "onboarding",
    });
  });
});
