// @vitest-environment jsdom

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentTask } from "@patchbay/core/types";
import { useAgentThreadPanelStore } from "@patchbay/core/agent-thread";

vi.mock("@patchbay/core/hooks", () => ({
  useWorkspaceId: () => "route-workspace",
}));

vi.mock("../../navigation", () => ({
  useNavigation: () => ({ pathname: "/acme/issues/ACME-1" }),
}));

import { AgentThreadButton } from "./agent-thread-button";

const task: AgentTask = {
  id: "task-from-older-server",
  agent_id: "agent-1",
  runtime_id: "runtime-1",
  issue_id: "",
  status: "completed",
  priority: 0,
  dispatched_at: null,
  started_at: null,
  completed_at: "2026-09-06T12:00:00Z",
  result: null,
  error: null,
  created_at: "2026-09-06T11:00:00Z",
};

describe("AgentThreadButton", () => {
  beforeEach(() => {
    useAgentThreadPanelStore.setState({ panel: null });
  });

  it("opens the right panel with the route workspace for older task payloads", async () => {
    render(
      <AgentThreadButton
        task={task}
        title="View conversation"
        renderButton={false}
        open
      />,
    );

    await waitFor(() => {
      expect(useAgentThreadPanelStore.getState().panel).toEqual({
        workspaceId: "route-workspace",
        taskId: "task-from-older-server",
        routePath: "/acme/issues/ACME-1",
      });
    });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("opens the selected task from its visible conversation button", () => {
    render(
      <AgentThreadButton
        task={task}
        title="View conversation"
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "View conversation" }));

    expect(useAgentThreadPanelStore.getState().panel?.taskId).toBe(
      "task-from-older-server",
    );
  });

  it("reports a controlled panel close without reopening it", async () => {
    const onOpenChange = vi.fn();
    render(
      <AgentThreadButton
        task={task}
        title="View conversation"
        renderButton={false}
        open
        onOpenChange={onOpenChange}
      />,
    );
    await waitFor(() => {
      expect(useAgentThreadPanelStore.getState().panel?.taskId).toBe(task.id);
    });

    act(() => useAgentThreadPanelStore.getState().closePanel());

    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
    expect(useAgentThreadPanelStore.getState().panel).toBeNull();
  });
});
