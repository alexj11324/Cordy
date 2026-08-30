import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import { chatKeys } from "@/data/queries/chat";
import { agentThreadKeys } from "@/data/queries/agent-thread";
import { invalidateAgentThreadContinuationQueries } from "./agent-thread";

describe("mobile Agent thread continuation cache invalidation", () => {
  it("refreshes the stable opener prefix after a child task is queued", () => {
    const queryClient = new QueryClient();
    const invalidate = vi.spyOn(queryClient, "invalidateQueries");

    invalidateAgentThreadContinuationQueries(
      queryClient,
      "workspace-1",
      "task-1",
      "task-2",
    );

    expect(invalidate).toHaveBeenCalledWith({
      queryKey: agentThreadKeys.all("workspace-1"),
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: agentThreadKeys.task("workspace-1", "task-1"),
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: chatKeys.taskMessages("task-1"),
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: chatKeys.taskMessages("task-2"),
    });
  });
});
