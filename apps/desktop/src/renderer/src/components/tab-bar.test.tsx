import { afterAll, describe, expect, it, vi, beforeEach } from "vitest";
import {
  render,
  renderHook,
  fireEvent,
  waitFor,
} from "@testing-library/react";
import { useScrollFade } from "@orvilo/ui/hooks/use-scroll-fade";

type MockTab = {
  id: string;
  url: string;
  title: string;
};

const state = vi.hoisted(() => ({
  activeWorkspaceSlug: "acme" as string | null,
  byWorkspace: {
    acme: {
      activeTabId: "tA",
      tabs: [
        { id: "tA", url: "/acme/issues", title: "Issues" },
        { id: "tB", url: "/acme/projects", title: "Projects" },
      ] as MockTab[],
    },
  } as Record<string, { activeTabId: string; tabs: MockTab[] }>,
  closeTab: vi.fn<(tabId: string) => void>(),
  closeOtherTabs: vi.fn<(tabId: string) => void>(),
  setActiveTab: vi.fn<(tabId: string) => void>(),
  moveTab: vi.fn<(from: number, to: number) => void>(),
  addTab: vi.fn<(path: string, title: string) => string>(),
  updateTab: vi.fn<(tabId: string, patch: { title?: string }) => void>(),
  openIssueWindow: vi.fn(),
}));

vi.mock("@/stores/tab-store", () => {
  const store = {
    get activeWorkspaceSlug() {
      return state.activeWorkspaceSlug;
    },
    get byWorkspace() {
      return state.byWorkspace;
    },
    closeTab: state.closeTab,
    closeOtherTabs: state.closeOtherTabs,
    setActiveTab: state.setActiveTab,
    moveTab: state.moveTab,
    addTab: state.addTab,
    updateTab: state.updateTab,
  };
  const useTabStore = Object.assign(
    (selector?: (s: typeof store) => unknown) =>
      selector ? selector(store) : store,
    { getState: () => store },
  );
  const useActiveGroup = () =>
    state.activeWorkspaceSlug
      ? (state.byWorkspace[state.activeWorkspaceSlug] ?? null)
      : null;
  return { useTabStore, useActiveGroup };
});

vi.mock("@orvilo/core/paths", async (importOriginal) => ({
  // Spread the real module so pure helpers (parseTabSubject etc.) keep working.
  ...(await importOriginal<typeof import("@orvilo/core/paths")>()),
  paths: {
    workspace: (slug: string) => ({
      issues: () => `/${slug}/issues`,
    }),
  },
}));

// The tab bar's presentation (URL/cache → visual + title) is covered by the
// views tab-presentation tests. Here we stub it so the strip-behavior tests
// (overflow, close, context menu) don't need the whole query stack. By
// default `title` mirrors the persisted tab title (matching the real hook's
// fallback); a test can set `pres.title` to simulate a resolved title that
// differs, to exercise the active-tab persist effect.
const pres = vi.hoisted(() => ({ title: null as string | null }));
vi.mock("@orvilo/views/layout", () => ({
  useTabPresentation: (_url: string, fallbackTitle?: string) => ({
    visual: { kind: "icon", icon: "ListTodo" },
    title: pres.title ?? fallbackTitle ?? "",
  }),
  ResourceLeadingVisual: ({ visual }: { visual: { kind: string; icon?: string } }) => (
    <span data-testid="tab-leading" data-visual-kind={visual.kind} data-icon={visual.icon} />
  ),
}));

import { TabBar } from "./tab-bar";

function reset() {
  state.activeWorkspaceSlug = "acme";
  state.byWorkspace = {
    acme: {
      activeTabId: "tA",
      tabs: [
        { id: "tA", url: "/acme/issues", title: "Issues" },
        { id: "tB", url: "/acme/projects", title: "Projects" },
      ],
    },
  };
  state.closeTab.mockReset();
  state.closeOtherTabs.mockReset();
  state.setActiveTab.mockReset();
  state.moveTab.mockReset();
  state.addTab.mockReset();
  state.updateTab.mockReset();
  state.openIssueWindow.mockReset();
  pres.title = null;
}

beforeEach(() => {
  reset();
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }));
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  vi.stubGlobal("desktopAPI", {
    openIssueWindow: state.openIssueWindow,
  });
});

afterAll(() => vi.unstubAllGlobals());

describe("TabBar actions", () => {
  // MUL-4370: each tab renders the shared ResourceLeadingVisual keyed off its
  // URL. (Which visual/title a URL resolves to is covered by the views
  // tab-presentation tests; here we only assert the strip wires it in.)
  it("renders the resource leading visual for every tab", () => {
    state.byWorkspace.acme.tabs = [
      { id: "tA", url: "/acme/automations", title: "Automations" },
      { id: "tB", url: "/acme/projects/proj-1", title: "Project" },
    ];
    const { getByLabelText } = render(<TabBar />);
    expect(
      getByLabelText("Automations").querySelector('[data-testid="tab-leading"]'),
    ).toBeTruthy();
    expect(
      getByLabelText("Project").querySelector('[data-testid="tab-leading"]'),
    ).toBeTruthy();
  });

});

describe("TabBar active-tab title persistence", () => {
  it("persists the resolved title only for the active tab", () => {
    pres.title = "MUL-1: Fixed";
    state.byWorkspace.acme.tabs = [
      { id: "tA", url: "/acme/issues/i1", title: "Issues" },
      { id: "tB", url: "/acme/projects", title: "Projects" },
    ];
    state.byWorkspace.acme.activeTabId = "tA";
    render(<TabBar />);
    expect(state.updateTab).toHaveBeenCalledWith("tA", { title: "MUL-1: Fixed" });
    expect(state.updateTab).not.toHaveBeenCalledWith("tB", expect.anything());
    expect(state.updateTab).toHaveBeenCalledTimes(1);
  });

  it("does not re-persist when the resolved title already matches (no loop)", () => {
    // Active tab's persisted title equals the resolved title.
    pres.title = "Issues";
    state.byWorkspace.acme.activeTabId = "tA"; // tA.title === "Issues"
    render(<TabBar />);
    expect(state.updateTab).not.toHaveBeenCalled();
  });
});

describe("TabBar rectangular surfaces", () => {
  it("renders the active tab as one detached rectangular surface", () => {
    const { getByLabelText } = render(<TabBar />);
    const surface = getByLabelText("Issues")
      .closest("[data-tab-frame]")
      ?.querySelector("[data-tab-surface]");

    expect(surface).toHaveClass(
      "rounded-sm",
      "border-surface-border",
      "bg-page-canvas",
      "shadow-sm",
    );
  });

  it("keeps inactive tabs rectangular and transparent until hover", () => {
    const { getByLabelText } = render(<TabBar />);
    const surface = getByLabelText("Projects")
      .closest("[data-tab-frame]")
      ?.querySelector("[data-tab-surface]");

    expect(surface).toHaveClass(
      "rounded-sm",
      "border-transparent",
      "bg-transparent",
      "group-hover/tab:bg-sidebar-accent",
    );
  });

  it("does not render Chrome-style flares or merged caps", () => {
    const { getByLabelText } = render(<TabBar />);
    const frame = getByLabelText("Issues").closest("[data-tab-frame]");
    expect(frame?.querySelector('[style*="radial-gradient"]')).toBeNull();
    expect(frame?.querySelector(".rounded-t-lg")).toBeNull();
  });
});

describe("TabBar overflow", () => {
  it("keeps tabs readable in a bounded horizontal scroller", () => {
    state.byWorkspace.acme.tabs = Array.from({ length: 8 }, (_, index) => ({
      id: `t${index}`,
      url: `/acme/tab-${index}`,
      title: `Tab ${index}`,
    }));

    const { container, getByLabelText } = render(<TabBar />);
    const tabBar = container.firstElementChild;
    const tabScroller = container.querySelector("[data-tab-scroll-container]");

    expect(tabBar).toHaveClass("min-w-0", "max-w-full");
    expect(tabScroller).toHaveClass(
      "min-w-0",
      "no-scrollbar",
      "overflow-x-auto",
      "overflow-y-hidden",
    );
    expect(getByLabelText("Tab 1").closest("[data-tab-frame]")).toHaveClass(
      "w-fit",
      "min-w-28",
      "max-w-48",
      "h-10",
    );

    const newTabButton = getByLabelText("New tab");
    expect(tabScroller).not.toContainElement(newTabButton);
  });

  it("uses a directional mask instead of a visible scrollbar", async () => {
    const tabScroller = document.createElement("div");
    Object.defineProperties(tabScroller, {
      clientWidth: { configurable: true, value: 320 },
      scrollWidth: { configurable: true, value: 960 },
    });
    const tabScrollRef = { current: tabScroller };
    const { result } = renderHook(() =>
      useScrollFade(tabScrollRef, 24, "horizontal"),
    );

    tabScroller.scrollLeft = 0;
    fireEvent.scroll(tabScroller);

    await waitFor(() => {
      expect(result.current?.maskImage).toBe(
        "linear-gradient(to right, black 0%, black calc(100% - 24px), transparent 100%)",
      );
    });

    tabScroller.scrollLeft = 240;
    fireEvent.scroll(tabScroller);

    await waitFor(() => {
      expect(result.current?.maskImage).toBe(
        "linear-gradient(to right, transparent 0%, black 24px, black calc(100% - 24px), transparent 100%)",
      );
    });
  });

  it("scrolls only the tab strip when the active tab moves out of view", () => {
    state.byWorkspace.acme.tabs = Array.from({ length: 6 }, (_, index) => ({
      id: `t${index}`,
      url: `/acme/tab-${index}`,
      title: `Tab ${index}`,
    }));
    state.byWorkspace.acme.activeTabId = "t0";

    const { container, getByLabelText, rerender } = render(<TabBar />);
    const tabScroller = container.querySelector(
      "[data-tab-scroll-container]",
    ) as HTMLDivElement;
    const lastTab = getByLabelText("Tab 5");

    vi.spyOn(tabScroller, "getBoundingClientRect").mockReturnValue({
      left: 100,
      right: 420,
    } as DOMRect);
    vi.spyOn(lastTab, "getBoundingClientRect").mockReturnValue({
      left: 450,
      right: 578,
    } as DOMRect);
    Object.defineProperties(tabScroller, {
      clientWidth: { configurable: true, value: 320 },
      scrollWidth: { configurable: true, value: 960 },
    });
    tabScroller.scrollLeft = 40;

    state.byWorkspace.acme.activeTabId = "t5";
    rerender(<TabBar />);

    expect(tabScroller.scrollLeft).toBe(222);
  });

  it("smoothly reveals a newly added active tab", () => {
    state.byWorkspace.acme.tabs = Array.from({ length: 6 }, (_, index) => ({
      id: `t${index}`,
      url: `/acme/tab-${index}`,
      title: `Tab ${index}`,
    }));
    state.byWorkspace.acme.activeTabId = "t0";

    const rectSpy = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockImplementation(function (this: HTMLElement) {
        if (this.matches("[data-tab-scroll-container]")) {
          return { left: 100, right: 420 } as DOMRect;
        }
        if (this.matches('[data-tab-id="t6"]')) {
          return { left: 450, right: 578 } as DOMRect;
        }
        return { left: 120, right: 248 } as DOMRect;
      });

    const { container, getByLabelText, rerender } = render(<TabBar />);
    const tabScroller = container.querySelector(
      "[data-tab-scroll-container]",
    ) as HTMLDivElement;
    Object.defineProperties(tabScroller, {
      clientWidth: { configurable: true, value: 320 },
      scrollWidth: { configurable: true, value: 960 },
    });
    tabScroller.scrollLeft = 40;
    const scrollTo = vi.fn(({ left }: ScrollToOptions) => {
      if (typeof left === "number") tabScroller.scrollLeft = left;
    });
    Object.defineProperty(tabScroller, "scrollTo", {
      configurable: true,
      value: scrollTo,
    });

    state.byWorkspace.acme.tabs = [
      ...state.byWorkspace.acme.tabs,
      {
        id: "t6",
        url: "/acme/tab-6",
        title: "Tab 6",
      },
    ];
    state.byWorkspace.acme.activeTabId = "t6";
    rerender(<TabBar />);

    expect(getByLabelText("Tab 6")).toHaveAttribute(
      "data-tab-entering",
      "true",
    );
    expect(scrollTo).toHaveBeenCalledWith({ left: 222, behavior: "smooth" });
    rectSpy.mockRestore();
  });

  it("keeps background additions offscreen and acknowledges them at the edge", () => {
    state.byWorkspace.acme.tabs = Array.from({ length: 6 }, (_, index) => ({
      id: `t${index}`,
      url: `/acme/tab-${index}`,
      title: `Tab ${index}`,
    }));
    state.byWorkspace.acme.activeTabId = "t0";

    const rectSpy = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockImplementation(function (this: HTMLElement) {
        if (this.matches("[data-tab-scroll-container]")) {
          return { left: 100, right: 420 } as DOMRect;
        }
        if (this.matches('[data-tab-id="t6"]')) {
          return { left: 450, right: 578 } as DOMRect;
        }
        return { left: 120, right: 248 } as DOMRect;
      });

    const { container, rerender } = render(<TabBar />);
    const tabScroller = container.querySelector(
      "[data-tab-scroll-container]",
    ) as HTMLDivElement;
    Object.defineProperties(tabScroller, {
      clientWidth: { configurable: true, value: 320 },
      scrollWidth: { configurable: true, value: 960 },
    });
    tabScroller.scrollLeft = 40;
    const scrollTo = vi.fn();
    Object.defineProperty(tabScroller, "scrollTo", {
      configurable: true,
      value: scrollTo,
    });

    state.byWorkspace.acme.tabs = [
      ...state.byWorkspace.acme.tabs,
      {
        id: "t6",
        url: "/acme/tab-6",
        title: "Tab 6",
      },
    ];
    rerender(<TabBar />);

    expect(tabScroller.scrollLeft).toBe(40);
    expect(scrollTo).not.toHaveBeenCalled();
    expect(
      container.querySelector('[data-new-tab-edge-feedback="true"]'),
    ).toBeInTheDocument();
    rectSpy.mockRestore();
  });
});

describe("TabBar context menu", () => {
  it("opens an issue-detail tab as a dedicated window", async () => {
    state.byWorkspace.acme.tabs = [
      {
        id: "tA",
        url: "/acme/issues/issue-1?comment=comment-1",
        title: "MUL-1: Fix tabs",
      },
    ];

    const { findByText, getByLabelText } = render(<TabBar />);
    fireEvent.contextMenu(getByLabelText("MUL-1: Fix tabs"));
    fireEvent.click(await findByText("Open as new window"));

    expect(state.openIssueWindow).toHaveBeenCalledWith({
      path: "/acme/issues/issue-1?comment=comment-1",
      title: "MUL-1: Fix tabs",
    });
  });

  it("does not offer a dedicated window for non-issue tabs", async () => {
    state.byWorkspace.acme.tabs = [
      {
        id: "tA",
        url: "/acme/issues",
        title: "Issues",
      },
    ];

    const { getByLabelText, queryByText } = render(<TabBar />);
    fireEvent.contextMenu(getByLabelText("Issues"));
    expect(queryByText("Open as new window")).toBeNull();
  });

  it("closes other tabs from the context menu", async () => {
    state.byWorkspace.acme.tabs = [
      { id: "tA", url: "/acme/issues", title: "Issues" },
      { id: "tB", url: "/acme/projects", title: "Projects" },
      { id: "tC", url: "/acme/agents", title: "Agents" },
    ];

    const { findByText, getByLabelText } = render(<TabBar />);
    fireEvent.contextMenu(getByLabelText("Projects"));
    fireEvent.click(await findByText("Close other tabs"));

    expect(state.closeOtherTabs).toHaveBeenCalledWith("tB");
  });

});
