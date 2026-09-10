import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { act, fireEvent, render } from "@testing-library/react";
import { useWindowOverlayStore } from "@/stores/window-overlay-store";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nProvider } from "@orvilo/core/i18n/react";
import { useSidebar } from "@orvilo/ui/components/ui/sidebar";
import { RESOURCES } from "@orvilo/views/locales";
import {
  TRAFFIC_LIGHT_CLUSTER_END,
  TRAFFIC_LIGHT_CONTENT_GAP,
  TRAFFIC_LIGHT_CONTENT_INSET,
} from "../../../shared/window-chrome";

// The shell resolves the mocked `getCurrentSlug()` against the workspace list
// before mounting workspace-scoped chrome, so the list has to contain it or
// the sidebar under test never renders. Gating behaviour itself is covered by
// desktop-layout.workspace-gate.test.tsx.
const WORKSPACES = [{ id: "ws-1", slug: "acme" }];

// The shell is the only thing under test here, so everything it mounts around
// the sidebar is stubbed out. What survives is the pair that has to agree:
// the title-row pin, and the `hasExternalTrigger` the provider publishes to
// every page header inside the canvas.
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

vi.mock("@orvilo/core/paths", () => ({
  WorkspaceSlugProvider: ({ children }: { children: ReactNode }) => (
    <>{children}</>
  ),
  paths: { workspace: () => ({ inbox: () => "/acme/inbox" }) },
  useCurrentWorkspace: () => null,
}));

vi.mock("@orvilo/core/platform", () => ({
  getCurrentSlug: () => "acme",
  subscribeToCurrentSlug: () => () => {},
}));

vi.mock("@orvilo/core/workspace", () => ({
  workspaceListOptions: () => ({
    queryKey: ["workspace-list"],
    queryFn: async () => WORKSPACES,
  }),
}));

vi.mock("@orvilo/views/navigation", () => ({
  useNavigation: () => ({ push: vi.fn() }),
}));

vi.mock("@orvilo/views/platform", () => ({
  useDesktopUnreadBadge: () => {},
}));

vi.mock("@orvilo/views/layout", () => ({
  AppSidebar: ({ topSlot }: { topSlot?: ReactNode }) => {
    const { open } = useSidebar();
    return <div data-slot="sidebar">{open ? topSlot : null}</div>;
  },
  GlobalShortcuts: () => null,
  NavigationProgress: () => null,
  ShellBreadcrumb: () => <nav aria-label="breadcrumb" data-testid="shell-breadcrumb" />,
  ShellHeaderProvider: ({ children }: { children: ReactNode }) => <>{children}</>,
  ShellHeaderActionsSlot: () => <div data-slot="shell-header-actions" />,
}));

vi.mock("@orvilo/views/modals/registry", () => ({ ModalRegistry: () => null }));
vi.mock("@orvilo/views/search", () => ({
  SearchCommand: () => null,
  SearchTrigger: () => null,
}));
vi.mock("@orvilo/views/chat", () => ({ GlobalRightSidebar: () => null, GlobalRightSidebarToggle: () => <button aria-label="Open right sidebar" /> }));
vi.mock("@orvilo/views/agent-thread", () => ({
  AgentThreadPanelLayout: ({ children }: { children: ReactNode }) => <>{children}</>,
}));
vi.mock("./window-overlay", () => ({ WindowOverlay: () => null }));
vi.mock("./tab-bar", () => ({ TabBar: () => <div data-testid="tab-bar" /> }));

// Stands in for whatever page the active tab is showing. Reports the one fact
// a `PageHeader` reads before deciding to render its own fallback trigger.
vi.mock("./tab-content", () => ({
  TabContent: () => {
    const {
      hasExternalTrigger,
      hideHoverSidebar,
      open,
      revealHoverSidebar,
      state,
    } = useSidebar();
    return (
      <div
        data-testid="page-content"
        data-external-trigger={hasExternalTrigger}
        data-sidebar-open={open}
        data-sidebar-state={state}
        onPointerEnter={revealHoverSidebar}
        onPointerLeave={hideHoverSidebar}
      />
    );
  },
}));

const { DesktopShell } = await import("./desktop-layout");

function renderShell(
  os: "macos" | "windows" = "windows",
  host: "electron" | "browser" = "electron",
) {
  (
    window as unknown as { desktopAPI: Record<string, unknown> }
  ).desktopAPI = {
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
  // The window toolbar parks a trigger beside the traffic lights that never
  // scrolls away, so nothing inside the canvas may add a second one. Desktop
  // windows sit below `xl`, exactly the band where `PageHeader`'s fallback
  // trigger renders, so every page used to stack an identical icon 50px under
  // this one — and a third when a list/detail surface brought its own header
  // along (MUL-6218).
  it("keeps exactly one trigger and tells page headers not to add another", () => {
    const { container, getByTestId } = renderShell();

    expect(container.querySelectorAll("[data-slot='sidebar-trigger']")).toHaveLength(1);
    expect(getByTestId("page-content")).toHaveAttribute(
      "data-external-trigger",
      "true",
    );
  });

  it("toggles from pointerdown when the titlebar swallows the click", () => {
    const { container, getByTestId } = renderShell("macos");
    const trigger = container.querySelector<HTMLElement>(
      "[data-slot='sidebar-trigger']",
    )!;

    fireEvent.pointerDown(trigger, { button: 0 });
    expect(getByTestId("page-content")).toHaveAttribute("data-sidebar-open", "false");

    fireEvent.click(trigger, { detail: 1 });
    expect(getByTestId("page-content")).toHaveAttribute("data-sidebar-open", "false");
  });

  it("accepts keyboard activation after a swallowed pointer click", () => {
    const { container, getByTestId } = renderShell("macos");
    const trigger = container.querySelector<HTMLElement>("[data-slot='sidebar-trigger']")!;
    fireEvent.pointerDown(trigger, { button: 0 });
    expect(getByTestId("page-content")).toHaveAttribute("data-sidebar-open", "false");
    // Closing moves the trigger from the sidebar title row into MainTopBar;
    // reacquire the mounted element before simulating keyboard activation.
    const headerTrigger = container.querySelector<HTMLElement>(
      "header [data-slot='sidebar-trigger']",
    )!;
    fireEvent.click(headerTrigger, { detail: 0 });
    expect(getByTestId("page-content")).toHaveAttribute("data-sidebar-open", "true");
  });

  it("keeps the pin in the window toolbar while the sidebar is closed", () => {
    const { container, getByTestId } = renderShell("macos");
    const header = container.querySelector("header")!;
    const toolbar = container.querySelector<HTMLElement>(
      "[data-slot='window-toolbar']",
    )!;
    const trigger = container.querySelector<HTMLElement>(
      "[data-slot='sidebar-trigger']",
    )!;
    const content = getByTestId("page-content");

    expect(content).toHaveAttribute("data-sidebar-open", "true");
    expect(toolbar).toContainElement(trigger);
    expect(header.querySelector("[data-slot='sidebar-trigger']")).toBeNull();

    fireEvent.click(trigger);
    expect(content).toHaveAttribute("data-sidebar-open", "false");
    expect(header).not.toHaveStyle({ WebkitAppRegion: "drag" });
    expect(header.firstElementChild).toHaveClass("flex", "h-full", "shrink-0", "items-center");
    expect(header.firstElementChild).not.toHaveClass("pointer-events-none");
    expect(header).toContainElement(
      container.querySelector("[data-slot='sidebar-trigger']")!,
    );
    expect(header.querySelector("[data-slot='sidebar-trigger']")).not.toBeNull();
    expect(container.querySelectorAll("[data-slot='sidebar-trigger']")).toHaveLength(1);

    // Hover reveal is disabled for this shell; the header trigger remains the
    // only way to reopen the off-canvas sidebar.
    fireEvent.pointerEnter(content);
    expect(content).toHaveAttribute("data-sidebar-state", "collapsed");
    expect(header.querySelector("[data-slot='sidebar-trigger']")).not.toBeNull();
    expect(header).toContainElement(
      container.querySelector("[data-slot='sidebar-trigger']")!,
    );
    expect(container.querySelectorAll("[data-slot='sidebar-trigger']")).toHaveLength(1);
  });

  it("derives the pin offset from traffic-light geometry and keeps the canvas flush", () => {
    const { container, getByTestId } = renderShell("macos");
    const toolbar = container.querySelector<HTMLElement>(
      "[data-slot='window-toolbar']",
    )!;
    const trigger = container.querySelector<HTMLElement>(
      "[data-slot='sidebar-trigger']",
    )!;
    const inset = container.querySelector<HTMLElement>(
      "[data-slot='sidebar-inset']",
    )!;
    const wrapper = container.querySelector<HTMLElement>(
      "[data-slot='sidebar-wrapper']",
    )!;
    const header = container.querySelector("header")!;

    expect(toolbar.firstElementChild).toHaveStyle({
      width: `${TRAFFIC_LIGHT_CONTENT_INSET}px`,
    });
    expect(toolbar.className.split(/\s+/)).not.toContain("pointer-events-none");
    expect(trigger.className).not.toMatch(/rotate-0/);
    expect(trigger.className).not.toMatch(/transition-none/);
    expect(wrapper.style.getPropertyValue("--desktop-traffic-light-end")).toBe(
      `${TRAFFIC_LIGHT_CLUSTER_END}px`,
    );
    expect(wrapper.style.getPropertyValue("--desktop-content-gutter")).toBe(
      `${TRAFFIC_LIGHT_CONTENT_GAP}px`,
    );
    expect(inset.className.split(/\s+/)).toEqual(
      expect.arrayContaining(["rounded-none!", "ring-0!"]),
    );
    expect(inset.className.split(/\s+/)).not.toContain("m-0!");
    expect(inset.className.split(/\s+/)).not.toContain("ml-2!");
    expect(inset.className.split(/\s+/)).not.toContain("rounded-xl");
    expect(header.querySelector("nav")?.parentElement).toHaveClass("w-fit", "max-w-full");

    fireEvent.click(trigger);
    expect(getByTestId("page-content")).toHaveAttribute("data-sidebar-open", "false");
    expect(header.firstElementChild?.firstElementChild).toHaveStyle({
      width: `${TRAFFIC_LIGHT_CONTENT_INSET}px`,
    });
  });

  it("uses the collapsible column policy instead of a compact Sheet at 963px", () => {
    const previousWidth = window.innerWidth;
    Object.defineProperty(window, "innerWidth", {
      configurable: true,
      value: 963,
    });
    const { container, getByTestId } = renderShell("macos");
    const wrapper = container.querySelector<HTMLElement>(
      "[data-slot='sidebar-wrapper']",
    )!;
    const trigger = container.querySelector<HTMLElement>(
      "[data-slot='sidebar-trigger']",
    )!;

    expect(wrapper).toHaveAttribute("data-compact-behavior", "collapse");
    fireEvent.click(trigger);
    expect(getByTestId("page-content")).toHaveAttribute("data-sidebar-open", "false");
    expect(document.querySelector("[role='dialog']")).not.toBeInTheDocument();

    Object.defineProperty(window, "innerWidth", {
      configurable: true,
      value: previousWidth,
    });
  });

  it("keeps the desktop shell opaque on every host", () => {
    const desktop = renderShell("macos").container.querySelector<HTMLElement>(
      "[data-slot='sidebar-wrapper']",
    )!;

    expect(desktop).not.toHaveAttribute("data-sidebar-glass");
    expect(desktop).not.toHaveAttribute("data-native-vibrancy");
    expect(desktop).toHaveClass("bg-app-shell");
    expect(desktop.parentElement).toHaveClass("bg-app-shell");
  });
});

it("keeps the mounted dashboard visible under the settings dialog", () => {
  const { container, getByTestId } = renderShell();
  const dashboard = container.querySelector('[data-slot="desktop-dashboard"]')!;
  const content = getByTestId("page-content");

  act(() => useWindowOverlayStore.getState().open({
    type: "settings",
    path: "/acme/settings",
  }));
  expect(dashboard).toHaveAttribute("inert");
  expect(dashboard).not.toHaveClass("invisible");
  expect(content).toBeInTheDocument();

  act(() => useWindowOverlayStore.getState().close());
  expect(dashboard).not.toHaveAttribute("inert");
  expect(getByTestId("page-content")).toBe(content);
});
