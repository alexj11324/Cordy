import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import { chatKeys } from "../chat/queries";
import { issueKeys } from "../issues/queries";
import { agentThreadKeys } from "./queries";
import { invalidateAgentThreadContinuationQueries } from "./mutations";

describe("Agent thread continuation cache invalidation", () => {
  it("refreshes the stable opener prefix as well as both task timelines", () => {
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
      queryKey: agentThreadKeys.detail("workspace-1", "task-1"),
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: issueKeys.tasksAll(),
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: chatKeys.taskMessages("task-1"),
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: chatKeys.taskMessages("task-2"),
    });
  });

  it("skips the continuation timeline when there is none", () => {
    const queryClient = new QueryClient();
    const invalidate = vi.spyOn(queryClient, "invalidateQueries");

    invalidateAgentThreadContinuationQueries(
      queryClient,
      "workspace-1",
      "task-1",
      null,
    );

    expect(invalidate).toHaveBeenCalledTimes(4);
    expect(invalidate).not.toHaveBeenCalledWith({
      queryKey: chatKeys.taskMessages(undefined as unknown as string),
    });
  });
});
