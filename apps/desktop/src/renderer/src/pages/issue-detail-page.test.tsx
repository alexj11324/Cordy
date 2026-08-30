import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const state = vi.hoisted(() => ({
  routeId: "PB-1",
}));

vi.mock("react-router-dom", () => ({
  useParams: () => ({ id: state.routeId }),
}));

vi.mock("@patchbay/core/hooks", () => ({
  useWorkspaceId: () => "workspace-1",
}));

vi.mock("@patchbay/core/issues/canonical-id", () => ({
  useCanonicalIssue: () => ({
    issue: { identifier: "PB-1", title: "Titlebar" },
  }),
}));

vi.mock("@/hooks/use-document-title", () => ({
  useDocumentTitle: vi.fn(),
}));

vi.mock("@patchbay/views/issues/components", () => ({
  IssueDetailRoute: ({
    leadingAction,
    onDelete,
  }: {
    leadingAction?: React.ReactNode;
    onDelete?: () => void;
  }) => (
    <div data-testid="issue-detail-route" data-on-delete={Boolean(onDelete)}>
      {leadingAction}
    </div>
  ),
}));

import { IssueDetailPage } from "./issue-detail-page";

beforeEach(() => {
  state.routeId = "PB-1";
});

describe("IssueDetailPage integrated titlebar", () => {
  it("supplies a drag target to the dedicated issue window", () => {
    render(<IssueDetailPage onDelete={vi.fn()} />);

    const route = screen.getByTestId("issue-detail-route");
    expect(route).toHaveAttribute("data-on-delete", "true");
    expect(route.firstElementChild).toHaveClass("h-12", "w-28");
    expect(route.firstElementChild).toHaveAttribute("aria-hidden", "true");
  });

  it("does not inject titlebar spacing into the main window route", () => {
    render(<IssueDetailPage />);

    const route = screen.getByTestId("issue-detail-route");
    expect(route).toHaveAttribute("data-on-delete", "false");
    expect(route).toBeEmptyDOMElement();
  });
});
