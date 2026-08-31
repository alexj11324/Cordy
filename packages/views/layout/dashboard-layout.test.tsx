import type { ReactNode } from "react";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("@patchbay/ui/components/ui/sidebar", () => ({
  SidebarProvider: ({ children }: { children: ReactNode }) => <>{children}</>,
  SidebarInset: ({
    children,
    className,
  }: {
    children: ReactNode;
    className?: string;
  }) => <main className={className}>{children}</main>,
}));

vi.mock("../modals/registry", () => ({ ModalRegistry: () => null }));
vi.mock("../onboarding", () => ({ SourceBackfillModal: () => null }));
vi.mock("./app-sidebar", () => ({ AppSidebar: () => null }));
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

const { DashboardLayout } = await import("./dashboard-layout");

describe("DashboardLayout route viewport", () => {
  it("gives every web route a default vertical scroll owner without scrolling overlays", () => {
    render(
      <DashboardLayout extra={<div data-testid="overlay" />}>
        <div data-testid="route-content" />
      </DashboardLayout>,
    );

    const viewport = screen.getByTestId("web-route-scroll-viewport");
    expect(viewport).toHaveClass(
      "flex",
      "min-h-0",
      "flex-1",
      "flex-col",
      "overflow-y-auto",
      "overscroll-contain",
    );
    expect(viewport).toContainElement(screen.getByTestId("route-content"));
    expect(viewport).not.toContainElement(screen.getByTestId("overlay"));
  });
});
