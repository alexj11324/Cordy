import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nProvider } from "@patchbay/core/i18n/react";
import { useSidebar } from "@patchbay/ui/components/ui/sidebar";
import { RESOURCES } from "@patchbay/views/locales";
import { useWindowOverlayStore } from "@/stores/window-overlay-store";

// The shell resolves the mocked `getCurrentSlug()` against the workspace list
// before mounting workspace-scoped chrome, so the list has to contain it or
// the sidebar under test never renders. Gating behaviour itself is covered by
// desktop-layout.workspace-gate.test.tsx.
const WORKSPACES = [{ id: "ws-1", slug: "acme" }];

// The shell is the only thing under test here, so everything it mounts around
// the sidebar is stubbed out. What survives is the pair that has to agree:
// `WindowToolbar`'s own trigger, and the `hasExternalTrigger` the provider
// publishes to every page header inside the canvas.
vi.mock("@/hooks/use-tab-history", () => ({
  useTabHistory: () => ({
    canGoBack: false,
    canGoForward: false,
    goBack: vi.fn(),
    goForward: vi.fn(),
  }),
  useNavigationInputBindings: () => {},
}));

vi.mock("@/platform/navigation", () => ({
  DesktopNavigationProvider: ({ children }: { children: ReactNode }) => (
    <>{children}</>
  ),
  routeContentLinkPath: vi.fn(),
}));

vi.mock("@patchbay/core/paths", () => ({
  WorkspaceSlugProvider: ({ children }: { children: ReactNode }) => (
    <>{children}</>
  ),
  paths: { workspace: () => ({ inbox: () => "/acme/inbox" }) },
  useCurrentWorkspace: () => null,
}));

vi.mock("@patchbay/core/platform", () => ({
  getCurrentSlug: () => "acme",
  subscribeToCurrentSlug: () => () => {},
}));

vi.mock("@patchbay/core/workspace", () => ({
  workspaceListOptions: () => ({
    queryKey: ["workspace-list"],
    queryFn: async () => WORKSPACES,
  }),
}));

vi.mock("@patchbay/views/navigation", () => ({
  useNavigation: () => ({ push: vi.fn() }),
}));

vi.mock("@patchbay/views/platform", () => ({
  useDesktopUnreadBadge: () => {},
}));

vi.mock("@patchbay/views/layout", () => ({
  AppSidebar: () => null,
  GlobalShortcuts: () => null,
  NavigationProgress: () => null,
}));

vi.mock("@patchbay/views/modals/registry", () => ({
  ModalRegistry: () => null,
}));
vi.mock("@patchbay/views/search", () => ({
  SearchCommand: () => null,
  SearchTrigger: () => null,
}));
vi.mock("@patchbay/views/chat", () => ({ FloatingChat: () => null }));
vi.mock("./tab-bar", () => ({ TabBar: () => null }));
vi.mock("./window-overlay", () => ({ WindowOverlay: () => null }));

// Stands in for whatever page the active tab is showing. Reports the one fact
// a `PageHeader` reads before deciding to render its own fallback trigger.
vi.mock("./tab-content", () => ({
  TabContent: () => {
    const {
      hasExternalTrigger,
      open,
      state,
      toggleSidebar,
      revealHoverSidebar,
    } = useSidebar();
    return (
      <div
        data-testid="page-content"
        data-external-trigger={hasExternalTrigger}
        data-open={open}
        data-state={state}
      >
        <button data-testid="toggle-sidebar" onClick={toggleSidebar} />
        <button data-testid="reveal-sidebar" onClick={revealHoverSidebar} />
      </div>
    );
  },
}));

const { DesktopShell } = await import("./desktop-layout");

beforeEach(() => {
  useWindowOverlayStore.getState().close();
});

function renderShell(
  os: "macos" | "windows" = "macos",
  host: "electron" | "browser" = "electron",
) {
  (window as unknown as { desktopAPI: Record<string, unknown> }).desktopAPI = {
    host,
    appInfo: { version: "0.0.0-test", os },
    onNavigationGesture: () => () => {},
    onInboxOpen: () => () => {},
  };

  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  qc.setQueryData(["workspace-list"], WORKSPACES);

  return render(
    <QueryClientProvider client={qc}>
      <I18nProvider locale="en" resources={RESOURCES}>
        <DesktopShell />
      </I18nProvider>
    </QueryClientProvider>,
  );
}

describe("DesktopShell sidebar trigger", () => {
  it("makes the covered application inert while standalone Settings is open", () => {
    const { getByTestId } = renderShell();
    const underlay = getByTestId("desktop-application-underlay");

    act(() => {
      useWindowOverlayStore.getState().open({
        type: "settings",
        path: "/acme/settings",
      });
    });
    expect(underlay).toHaveAttribute("inert");
    expect(underlay).toHaveAttribute("aria-hidden", "true");

    act(() => useWindowOverlayStore.getState().close());
    expect(underlay).not.toHaveAttribute("inert");
    expect(underlay).not.toHaveAttribute("aria-hidden");
  });

  it("enables the glass shell while reserving native transparency for macOS", () => {
    const mac = renderShell("macos").container.querySelector<HTMLElement>(
      "[data-slot='sidebar-wrapper']",
    )!;

    expect(mac).toHaveAttribute("data-sidebar-glass", "true");
    expect(mac).toHaveAttribute("data-native-vibrancy", "true");
    expect(mac).toHaveClass("bg-transparent");
    expect(mac.parentElement).toHaveClass("bg-transparent");

    const windows = renderShell("windows").container.querySelector<HTMLElement>(
      "[data-slot='sidebar-wrapper']",
    )!;

    expect(windows).toHaveAttribute("data-sidebar-glass", "true");
    expect(windows).not.toHaveAttribute("data-native-vibrancy");
    expect(windows).toHaveClass("bg-app-shell");
    expect(windows.parentElement).toHaveClass("bg-app-shell");

    const browserMac = renderShell(
      "macos",
      "browser",
    ).container.querySelector<HTMLElement>("[data-slot='sidebar-wrapper']")!;

    expect(browserMac).toHaveAttribute("data-sidebar-glass", "true");
    expect(browserMac).not.toHaveAttribute("data-native-vibrancy");
    expect(browserMac).toHaveClass("bg-app-shell");
    expect(browserMac.parentElement).toHaveClass("bg-app-shell");
  });

  // The window toolbar parks a trigger beside the traffic lights that never
  // scrolls away, so nothing inside the canvas may add a second one. Desktop
  // windows sit below `xl`, exactly the band where `PageHeader`'s fallback
  // trigger renders, so every page used to stack an identical icon 50px under
  // this one — and a third when a list/detail surface brought its own header
  // along (PB-6218).
  it("keeps exactly one trigger and tells page headers not to add another", () => {
    const { container, getByTestId } = renderShell();

    expect(
      container.querySelectorAll("[data-slot='sidebar-trigger']"),
    ).toHaveLength(1);
    expect(getByTestId("page-content")).toHaveAttribute(
      "data-external-trigger",
      "true",
    );
  });

  it("keeps desktop geometry collapsed during a temporary hover reveal", async () => {
    const { container, getByTestId } = renderShell();
    const topBar = container.querySelector("header");
    const canvas = getByTestId("page-content").parentElement;

    expect(topBar).toHaveStyle({ paddingLeft: "0px" });
    expect(canvas).toHaveStyle({ marginLeft: "2px" });

    fireEvent.click(getByTestId("toggle-sidebar"));
    await waitFor(() => {
      expect(getByTestId("page-content")).toHaveAttribute("data-open", "false");
      expect(topBar).toHaveStyle({ paddingLeft: "184px" });
      expect(canvas).toHaveStyle({ marginLeft: "8px" });
    });

    fireEvent.click(getByTestId("reveal-sidebar"));
    await waitFor(() => {
      expect(getByTestId("page-content")).toHaveAttribute(
        "data-state",
        "expanded",
      );
      expect(getByTestId("page-content")).toHaveAttribute("data-open", "false");
    });
    expect(topBar).toHaveStyle({ paddingLeft: "184px" });
    expect(canvas).toHaveStyle({ marginLeft: "8px" });
  });
});
