import type { ReactNode } from "react";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const coordinator = vi.hoisted(() => ({
  registerActiveHostElement: vi.fn(),
}));

vi.mock("@tanstack/react-query", () => ({
  useQueryClient: () => ({}),
}));

vi.mock("react-router-dom", () => ({
  RouterProvider: () => <div data-testid="router-content" />,
}));

vi.mock("@patchbay/views/platform", () => ({
  ScrollRestorationProvider: ({ children }: { children: ReactNode }) => (
    <>{children}</>
  ),
}));

vi.mock("@/stores/tab-store", () => ({
  useActiveGroup: () => ({
    activeTabId: "tab-1",
    tabs: [{ id: "tab-1", title: "Issues" }],
  }),
  useTabStore: (selector: (state: { mountGeneration: number }) => number) =>
    selector({ mountGeneration: 0 }),
}));

vi.mock("@/platform/tab-coordinator", () => ({
  createScrollRestorationAdapter: () => ({}),
  getAppRouter: () => ({}),
  initTabCoordinator: vi.fn(),
  registerActiveHostElement: coordinator.registerActiveHostElement,
  registerCoordinatorQueryClient: vi.fn(),
}));

const { TabContent } = await import("./tab-content");

describe("TabContent route viewport", () => {
  it("gives every desktop route a default vertical scroll owner", () => {
    render(<TabContent />);

    const viewport = screen.getByTestId("desktop-route-scroll-viewport");
    expect(viewport).toHaveClass(
      "flex",
      "min-h-0",
      "flex-1",
      "flex-col",
      "overflow-y-auto",
      "overscroll-contain",
    );
    expect(viewport).toHaveAttribute("data-tab-scroll-root", "route");
    expect(viewport).toContainElement(screen.getByTestId("router-content"));
  });
});
