import { beforeEach, describe, expect, it } from "vitest";
import { useAgentThreadPanelStore } from "./panel-store";

describe("Agent thread panel store", () => {
  beforeEach(() => {
    useAgentThreadPanelStore.setState({ panel: null });
  });

  it("opens one task-scoped panel and replaces it with the next selection", () => {
    useAgentThreadPanelStore.getState().openPanel({
      workspaceId: "workspace-1",
      taskId: "task-1",
      routePath: "/acme/issues/ACME-1",
    });
    useAgentThreadPanelStore.getState().openPanel({
      workspaceId: "workspace-1",
      taskId: "task-2",
      routePath: "/acme/issues/ACME-1",
    });

    expect(useAgentThreadPanelStore.getState().panel).toEqual({
      workspaceId: "workspace-1",
      taskId: "task-2",
      routePath: "/acme/issues/ACME-1",
    });
  });

  it("closes without retaining a stale task selection", () => {
    useAgentThreadPanelStore.getState().openPanel({
      workspaceId: "workspace-1",
      taskId: "task-1",
      routePath: "/acme/issues/ACME-1",
    });

    useAgentThreadPanelStore.getState().closePanel();

    expect(useAgentThreadPanelStore.getState().panel).toBeNull();
  });
});
