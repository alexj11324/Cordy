// @vitest-environment jsdom

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAgentThreadPanelStore } from "@patchbay/core/agent-thread";

const context = vi.hoisted(() => ({
  workspaceId: "workspace-1" as string | null,
  pathname: "/acme/issues/ACME-1",
}));

vi.mock("@patchbay/core/paths", () => ({
  useCurrentWorkspace: () => context.workspaceId
    ? { id: context.workspaceId, slug: "acme" }
    : undefined,
}));

vi.mock("../../navigation", () => ({
  useNavigation: () => ({ pathname: context.pathname }),
}));

vi.mock("../../i18n", () => ({
  useT: () => ({ t: () => "Resize Agent conversation" }),
}));

vi.mock("@patchbay/ui/components/ui/resizable", () => ({
  ResizablePanelGroup: ({ children }: { children: ReactNode }) => (
    <div data-testid="panel-group">{children}</div>
  ),
  ResizablePanel: ({
    children,
    id,
    defaultSize,
    minSize,
    maxSize,
  }: {
    children: ReactNode;
    id: string;
    defaultSize?: number | string;
    minSize?: number | string;
    maxSize?: number | string;
  }) => (
    <div
      data-testid={id}
      data-default-size={defaultSize}
      data-min-size={minSize}
      data-max-size={maxSize}
    >
      {children}
    </div>
  ),
  ResizableHandle: (props: Record<string, unknown>) => (
    <div role="separator" aria-label={String(props["aria-label"])} />
  ),
}));

vi.mock("./task-agent-thread-panel", () => ({
  TaskAgentThreadPanel: ({ taskId, onClose }: { taskId: string; onClose: () => void }) => (
    <aside data-testid="agent-thread-panel-content" data-task-id={taskId}>
      <button type="button" onClick={onClose}>Close conversation</button>
    </aside>
  ),
}));

import { AgentThreadPanelLayout } from "./agent-thread-panel-layout";

describe("AgentThreadPanelLayout", () => {
  beforeEach(() => {
    context.workspaceId = "workspace-1";
    context.pathname = "/acme/issues/ACME-1";
    useAgentThreadPanelStore.setState({ panel: null });
  });

  it("opens a constrained resizable right panel without remounting main content", () => {
    render(
      <AgentThreadPanelLayout>
        <input aria-label="Main page draft" defaultValue="" />
      </AgentThreadPanelLayout>,
    );
    const input = screen.getByRole("textbox", { name: "Main page draft" });
    fireEvent.change(input, { target: { value: "unfinished main-page draft" } });

    act(() => {
      useAgentThreadPanelStore.getState().openPanel({
        workspaceId: "workspace-1",
        taskId: "task-1",
        routePath: "/acme/issues/ACME-1",
      });
    });

    const panel = screen.getByTestId("agent-thread-sidebar");
    expect(panel).toHaveAttribute("data-default-size", "420");
    expect(panel).toHaveAttribute("data-min-size", "320");
    expect(panel).toHaveAttribute("data-max-size", "65%");
    expect(screen.getByRole("separator", { name: "Resize Agent conversation" }))
      .toBeInTheDocument();
    expect(input).toHaveValue("unfinished main-page draft");

    fireEvent.click(screen.getByRole("button", { name: "Close conversation" }));

    expect(screen.queryByTestId("agent-thread-sidebar")).not.toBeInTheDocument();
    expect(input).toHaveValue("unfinished main-page draft");
  });

  it("closes stale panel state after route or workspace changes", async () => {
    useAgentThreadPanelStore.getState().openPanel({
      workspaceId: "workspace-old",
      taskId: "task-old",
      routePath: "/old/issues/OLD-1",
    });

    render(
      <AgentThreadPanelLayout>
        <div>Main page</div>
      </AgentThreadPanelLayout>,
    );

    expect(screen.queryByTestId("agent-thread-sidebar")).not.toBeInTheDocument();
    await waitFor(() => {
      expect(useAgentThreadPanelStore.getState().panel).toBeNull();
    });
  });
});
