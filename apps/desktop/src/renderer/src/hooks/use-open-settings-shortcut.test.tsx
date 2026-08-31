// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "@testing-library/react";
import { useModalStore } from "@patchbay/core/modals";
import { useTabStore } from "@/stores/tab-store";
import { useWindowOverlayStore } from "@/stores/window-overlay-store";
import {
  openSettingsPage,
  useOpenSettingsShortcut,
} from "./use-open-settings-shortcut";

function Probe() {
  useOpenSettingsShortcut();
  return null;
}

/** One workspace with a single issues tab open. */
function seedTabs() {
  useTabStore.setState({
    activeWorkspaceSlug: "acme",
    byWorkspace: {
      acme: {
        tabs: [
          {
            id: "t1",
            url: "/acme/issues",
            resourceKey: "/acme/issues",
            title: "",
            pinned: false,
            history: { stack: ["/acme/issues"], index: 0 },
            memento: { scroll: {}, view: {} },
          },
        ],
        activeTabId: "t1",
        recentTabIds: ["t1"],
      },
    },
  });
}

function tabUrls(): string[] {
  return useTabStore.getState().byWorkspace.acme?.tabs.map((t) => t.url) ?? [];
}

/** Installs a desktopAPI stub; `deliver()` plays the chord main would send. */
function stubDesktopAPI(kind: "main" | "issue") {
  let handler: (() => void) | null = null;
  const onOpenSettings = vi.fn((callback: () => void) => {
    handler = callback;
    return () => {
      handler = null;
    };
  });
  (window as unknown as { desktopAPI: Record<string, unknown> }).desktopAPI = {
    windowContext:
      kind === "main"
        ? { kind: "main" }
        : { kind: "issue", path: "/acme/issues/abc", workspaceSlug: "acme" },
    onOpenSettings,
  };
  return { onOpenSettings, deliver: () => handler?.() };
}

describe("openSettingsPage", () => {
  beforeEach(() => {
    seedTabs();
    useWindowOverlayStore.setState({ overlay: null });
    useModalStore.getState().close();
  });

  it("opens Settings for the active workspace without changing tabs", () => {
    openSettingsPage();

    expect(tabUrls()).toEqual(["/acme/issues"]);
    expect(useWindowOverlayStore.getState().overlay).toEqual({
      type: "settings",
      path: "/acme/settings",
    });
  });

  it("keeps an already-open Settings page stable", () => {
    openSettingsPage();
    const first = useWindowOverlayStore.getState().overlay;
    openSettingsPage();

    expect(useWindowOverlayStore.getState().overlay).toBe(first);
    expect(tabUrls()).toEqual(["/acme/issues"]);
  });

  it("closes a portaled modal before opening Settings", () => {
    useModalStore.getState().open("create-issue");

    openSettingsPage();

    expect(useModalStore.getState().modal).toBeNull();
    expect(useWindowOverlayStore.getState().overlay).toEqual({
      type: "settings",
      path: "/acme/settings",
    });
  });

  it("does nothing while a pre-workspace overlay covers the window", () => {
    useWindowOverlayStore.setState({ overlay: { type: "onboarding" } });

    openSettingsPage();

    expect(tabUrls()).toEqual(["/acme/issues"]);
  });

  it("does nothing without an active workspace (logged out)", () => {
    useTabStore.setState({ activeWorkspaceSlug: null });

    expect(() => openSettingsPage()).not.toThrow();
    expect(tabUrls()).toEqual(["/acme/issues"]);
  });
});

describe("useOpenSettingsShortcut", () => {
  beforeEach(() => {
    seedTabs();
    useWindowOverlayStore.setState({ overlay: null });
    useModalStore.getState().close();
  });

  it("opens Settings when main delivers the chord", () => {
    const { deliver } = stubDesktopAPI("main");
    render(<Probe />);

    deliver();

    expect(useWindowOverlayStore.getState().overlay).toEqual({
      type: "settings",
      path: "/acme/settings",
    });
  });

  // Main routes the chord to the app window; an issue renderer that also
  // subscribed would mark the channel ready and drain the request into a
  // window that has no tabs to open it in.
  it("does not subscribe in a dedicated issue window", () => {
    const { onOpenSettings } = stubDesktopAPI("issue");

    render(<Probe />);

    expect(onOpenSettings).not.toHaveBeenCalled();
  });
});
