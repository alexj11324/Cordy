import type { ComponentType, CSSProperties, ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { act, fireEvent, render } from "@testing-library/react";
import { useWindowOverlayStore } from "@/stores/window-overlay-store";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nProvider } from "@patchbay/core/i18n/react";
import { useSidebar } from "@patchbay/ui/components/ui/sidebar";
import { RESOURCES } from "@patchbay/views/locales";

// Layout ownership is the contract under test. Apply Motion targets
// synchronously so the test asserts the intended endpoint rather than waiting
// on requestAnimationFrame-driven spring timing in jsdom.
vi.mock("motion/react", async () => {
  const React = await import("react");
  const components = new Map<PropertyKey, ComponentType<Record<string, unknown>>>();

  return {
    motion: new Proxy({}, {
      get: (_target, tag: PropertyKey) => {
        const cached = components.get(tag);
        if (cached) return cached;

        const Component = React.forwardRef<HTMLElement, Record<string, unknown>>(
          ({ animate, initial: _initial, transition: _transition, style, ...props }, ref) =>
            React.createElement(tag as string, {
              ...props,
              ref,
              style: { ...(style as CSSProperties), ...(animate as CSSProperties) },
            }),
        );
        Component.displayName = `motion.${String(tag)}`;
        components.set(tag, Component);
        return Component;
      },
    }),
  };
});

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

vi.mock("@patchbay/views/modals/registry", () => ({ ModalRegistry: () => null }));
vi.mock("@patchbay/views/search", () => ({
  SearchCommand: () => null,
  SearchTrigger: () => null,
}));
vi.mock("@patchbay/views/chat", () => ({ FloatingChat: () => null }));
vi.mock("@patchbay/views/agent-thread", () => ({
  AgentThreadPanelLayout: ({ children }: { children: ReactNode }) => <>{children}</>,
}));
vi.mock("./tab-bar", () => ({ TabBar: () => <div data-testid="tab-bar" /> }));
vi.mock("./window-overlay", () => ({ WindowOverlay: () => null }));

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

  it("keeps the native toolbar clearance while a collapsed sidebar is hover-revealed", () => {
    const { container, getByTestId } = renderShell("macos");
    const header = container.querySelector("header")!;
    const trigger = container.querySelector<HTMLElement>(
      "[data-slot='sidebar-trigger']",
    )!;
    const content = getByTestId("page-content");

    expect(content).toHaveAttribute("data-sidebar-open", "true");
    expect(header).toHaveStyle({ paddingLeft: "0px" });

    fireEvent.click(trigger);
    expect(content).toHaveAttribute("data-sidebar-open", "false");
    expect(header).toHaveStyle({ paddingLeft: "184px" });
    expect(container.querySelectorAll("[data-slot='sidebar-trigger']")).toHaveLength(1);

    fireEvent.pointerEnter(content);
    expect(content).toHaveAttribute("data-sidebar-state", "expanded");
    expect(header).toHaveStyle({ paddingLeft: "184px" });

    expect(container.querySelectorAll("[data-slot='sidebar-trigger']")).toHaveLength(1);
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

  // The macOS shell is transparent so Electron's native sidebar material can
  // show through; other platforms keep the opaque app-shell wrapper. The
  // marker is what the globals.css `:has()` gate keys off to drop the body
  // fill — without it the vibrancy stays buried under an opaque page.
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
  });

  it("keeps the opaque shell in browser hosts even on macOS", () => {
    const browser = renderShell("macos", "browser").container.querySelector<HTMLElement>(
      "[data-slot='sidebar-wrapper']",
    )!;

    expect(browser).toHaveAttribute("data-sidebar-glass", "true");
    expect(browser).not.toHaveAttribute("data-native-vibrancy");
    expect(browser).toHaveClass("bg-app-shell");
  });
});

it("hides the mounted dashboard while glass Settings owns the window", () => {
  const { container, getByTestId } = renderShell();
  const dashboard = container.querySelector('[data-slot="desktop-dashboard"]')!;
  const content = getByTestId("page-content");

  act(() => useWindowOverlayStore.getState().open({
    type: "settings",
    path: "/acme/settings",
  }));
  expect(dashboard).toHaveAttribute("inert");
  expect(dashboard).toHaveClass("invisible");
  expect(content).toBeInTheDocument();

  act(() => useWindowOverlayStore.getState().close());
  expect(dashboard).not.toHaveAttribute("inert");
  expect(dashboard).not.toHaveClass("invisible");
  expect(getByTestId("page-content")).toBe(content);
});
