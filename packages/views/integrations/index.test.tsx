// @vitest-environment jsdom

import { describe, expect, it, vi } from "vitest";
import "@testing-library/jest-dom/vitest";
import { render, screen } from "@testing-library/react";
import { Button } from "@lobehub/ui/base-ui";

vi.mock("../settings/components/integrations-tab", () => ({
  IntegrationsTab: ({ standalone }: { standalone?: boolean }) => (
    <div data-testid="integrations-tab" data-standalone={String(standalone)}>
      {/*
        A real Lobe primitive, deliberately. This page is one of the hosts of
        `IntegrationsTab`, and its channel dialog renders `SlackTab` /
        `TelegramTab`, which are Lobe since Task 7a. Every Lobe primitive that
        animates calls `useMotionComponent()` and throws
        `Please wrap your app with <ConfigProvider> (or <MotionProvider>)`
        without a bridge — so a stub that renders one turns "this page provides
        the bridge" into something the suite can actually fail on. Without it,
        removing the bridge from `index.tsx` would leave every test here green.
      */}
      <Button>Lobe probe</Button>
    </div>
  ),
}));

// Exercise the same public entry point imported by the Web route.
import { WorkspaceIntegrationsPage } from "@orvilo/views/integrations";

describe("WorkspaceIntegrationsPage", () => {
  it("delegates scrolling to the shared route viewport", () => {
    render(<WorkspaceIntegrationsPage />);

    expect(
      screen.queryByTestId("workspace-integrations-scroller"),
    ).not.toBeInTheDocument();
    expect(screen.getByTestId("integrations-tab")).toHaveAttribute(
      "data-standalone",
      "true",
    );
  });

  it("mounts the Lobe theme bridge its tab content needs", () => {
    render(<WorkspaceIntegrationsPage />);

    // Reaching the assertion at all is the point: without the bridge this
    // render throws before there is anything to query.
    expect(screen.getByRole("button", { name: "Lobe probe" })).toBeInTheDocument();
  });
});
