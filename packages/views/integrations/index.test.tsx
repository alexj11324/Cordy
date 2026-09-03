// @vitest-environment jsdom

import { describe, expect, it, vi } from "vitest";
import "@testing-library/jest-dom/vitest";
import { render, screen } from "@testing-library/react";

vi.mock("../settings/components/integrations-tab", () => ({
  IntegrationsTab: ({ standalone }: { standalone?: boolean }) => (
    <div data-testid="integrations-tab" data-standalone={String(standalone)} />
  ),
}));

import { WorkspaceIntegrationsPage } from "./index";

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
});
