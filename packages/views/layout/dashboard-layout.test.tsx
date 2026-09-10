import type { ReactNode } from "react";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("@orvilo/ui/components/ui/sidebar", () => ({
  SidebarProvider: ({ children }: { children: ReactNode }) => <>{children}</>,
  SidebarInset: ({
    children,
    className,
  }: {
    children: ReactNode;
    className?: string;
  }) => <main className={className}>{children}</main>,
  SidebarTrigger: () => <button type="button" data-slot="sidebar-trigger" />,
}));

vi.mock("../chat/global-right-sidebar", () => ({
  GlobalRightSidebar: () => <aside data-testid="global-right-sidebar" />,
  GlobalRightSidebarToggle: () => <button aria-label="Open right sidebar" />,
}));

vi.mock("../modals/registry", () => ({ ModalRegistry: () => null }));
vi.mock("../onboarding", () => ({ SourceBackfillModal: () => null }));
vi.mock("./app-sidebar", () => ({ AppSidebar: () => null }));
vi.mock("./shell-breadcrumb", () => ({ ShellBreadcrumb: () => null }));
vi.mock("./dashboard-guard", () => ({
  DashboardGuard: ({ children }: { children: ReactNode }) => <>{children}</>,
}));
vi.mock("./navigation-progress", () => ({
  NavigationProgress: () => null,
}));
vi.mock("./workspace-presence-prefetch", () => ({
  WorkspacePresencePrefetch: () => null,
}));
vi.mock("./global-shortcuts", () => ({ GlobalShortcuts: () => null }));
vi.mock("../agent-thread/components/agent-thread-panel-layout", () => ({
  AgentThreadPanelLayout: ({ children }: { children: ReactNode }) => (
    <div data-testid="agent-thread-layout">{children}</div>
  ),
}));

const { DashboardLayout } = await import("./dashboard-layout");

describe("DashboardLayout route viewport", () => {
  it("gives every web route a default vertical scroll owner without scrolling overlays", () => {
    render(
      <DashboardLayout extra={<div data-testid="overlay" />}>
        <div data-testid="route-content" />
      </DashboardLayout>,
    );

    const viewport = screen.getByTestId("web-route-scroll-viewport");
    const mainColumn = viewport.closest("[data-slot=shell-main-column]");
    const sidebar = screen.getByTestId("global-right-sidebar");
    expect(mainColumn?.parentElement).toBe(sidebar.parentElement);
    expect(mainColumn?.querySelector("header")).not.toBeNull();
    expect(mainColumn).not.toContainElement(sidebar);
    expect(viewport).toHaveClass(
      "flex",
      "min-h-0",
      "flex-1",
      "flex-col",
      "overflow-y-auto",
      "overscroll-contain",
    );
    expect(viewport).toContainElement(screen.getByTestId("route-content"));
    expect(screen.getByTestId("agent-thread-layout")).toContainElement(viewport);
    expect(viewport).not.toContainElement(screen.getByTestId("overlay"));
    expect(
      document.querySelector("[data-slot='shell-header-actions']"),
    ).not.toBeNull();
  });
});
