import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const state = vi.hoisted(() => ({
  workspaces: [] as { id: string; slug: string }[],
  ready: false,
  unavailable: false,
  isFetching: false,
  refetch: vi.fn(),
}));

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: (selector: (auth: { user: { id: string } }) => unknown) =>
    selector({ user: { id: "user-1" } }),
}));

vi.mock("@patchbay/core/workspace", () => ({
  useWorkspaceList: () => ({
    workspaces: state.workspaces,
    ready: state.ready,
    unavailable: state.unavailable,
    isFetching: state.isFetching,
    refetch: state.refetch,
  }),
}));

vi.mock("@patchbay/core/platform", () => ({
  setCurrentWorkspace: vi.fn(),
}));

vi.mock("@patchbay/core/paths", () => ({
  WorkspaceSlugProvider: ({ children }: { children: React.ReactNode }) => (
    <>{children}</>
  ),
}));

vi.mock("@patchbay/ui/components/common/patchbay-icon", () => ({
  PatchbayIcon: () => <div data-testid="workspace-loading" />,
}));

vi.mock("@patchbay/views/modals/registry", () => ({
  ModalRegistry: () => null,
}));

vi.mock("@patchbay/views/layout", () => ({
  WorkspacePresencePrefetch: () => null,
}));

vi.mock("@patchbay/views/platform", () => ({
  DragStrip: () => <div data-testid="drag-strip" />,
}));

vi.mock("../pages/issue-detail-page", () => ({
  IssueDetailPage: () => <div data-testid="issue-detail" />,
}));

vi.mock("../pages/auth-recovery", () => ({
  DesktopAuthRecoveryPage: ({
    onRetry,
    isRetrying,
  }: {
    onRetry: () => void;
    isRetrying: boolean;
  }) => (
    <button type="button" disabled={isRetrying} onClick={onRetry}>
      Retry workspace list
    </button>
  ),
}));

vi.mock("../platform/issue-window-navigation", () => ({
  IssueWindowNavigationProvider: ({
    children,
  }: {
    children: React.ReactNode;
  }) => <>{children}</>,
}));

import { IssueWindow } from "./issue-window";

const context = {
  kind: "issue" as const,
  path: "/acme/issues/PB-1",
  title: "PB-1",
  workspaceSlug: "acme",
  issueId: "PB-1",
};

beforeEach(() => {
  state.workspaces = [];
  state.ready = false;
  state.unavailable = false;
  state.isFetching = false;
  state.refetch.mockReset();
});

describe("IssueWindow", () => {
  it("lets the issue header own the integrated titlebar", async () => {
    state.ready = true;
    state.workspaces = [{ id: "ws-1", slug: "acme" }];

    render(<IssueWindow context={context} />);

    expect(await screen.findByTestId("issue-detail")).toBeInTheDocument();
    expect(screen.queryByTestId("drag-strip")).not.toBeInTheDocument();
  });

  it("keeps loading after an initial workspace-list failure", async () => {
    render(<IssueWindow context={context} />);

    expect(await screen.findByTestId("workspace-loading")).toBeInTheDocument();
    expect(screen.getByTestId("drag-strip")).toBeInTheDocument();
    expect(screen.queryByText("Issue unavailable")).toBeNull();
  });

  it("shows unavailable only after an authoritative list omits the workspace", async () => {
    state.ready = true;

    render(<IssueWindow context={context} />);

    expect(await screen.findByText("Issue unavailable")).toBeInTheDocument();
    expect(screen.queryByTestId("workspace-loading")).toBeNull();
  });

  it("offers a retry when the initial workspace-list request fails", async () => {
    state.unavailable = true;

    render(<IssueWindow context={context} />);

    const retry = await screen.findByRole("button", {
      name: "Retry workspace list",
    });
    fireEvent.click(retry);
    expect(state.refetch).toHaveBeenCalledOnce();
    expect(screen.queryByTestId("workspace-loading")).toBeNull();
  });
});
