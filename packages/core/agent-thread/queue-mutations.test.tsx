// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiClient, setApiInstance } from "../api";
import { agentThreadKeys } from "./queries";
import { useSteerAgentThread } from "./queue-mutations";

const taskResponse = (id: string) => ({
  id,
  agent_id: "agent-1",
  runtime_id: "runtime-1",
  issue_id: "issue-1",
  status: "cancelled",
  priority: 4,
  dispatched_at: null,
  started_at: null,
  completed_at: "2026-09-06T12:00:00Z",
  result: null,
  error: null,
  created_at: "2026-09-06T11:00:00Z",
});

function wrapper(queryClient: QueryClient) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

afterEach(() => vi.unstubAllGlobals());

describe("Agent thread steering", () => {
  it("prioritizes the selected queued task then interrupts the server-selected active task", async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({
        task_id: "queued-task",
        active_task_id: "running-task",
      }), { status: 200, headers: { "Content-Type": "application/json" } }))
      .mockResolvedValueOnce(new Response(JSON.stringify(taskResponse("running-task")), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }));
    vi.stubGlobal("fetch", fetchMock);
    setApiInstance(new ApiClient("https://api.example.test"));
    const queryClient = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
    const invalidate = vi.spyOn(queryClient, "invalidateQueries");
    const { result } = renderHook(
      () => useSteerAgentThread("workspace-1", "opener-task"),
      { wrapper: wrapper(queryClient) },
    );

    await act(async () => {
      await result.current.mutateAsync("queued-task");
    });

    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
      "https://api.example.test/api/tasks/opener-task/agent-thread/queued-tasks/queued-task/prioritize",
      "https://api.example.test/api/tasks/running-task/cancel",
    ]);
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: agentThreadKeys.all("workspace-1"),
    });
  });

  it("propagates permission failures without interrupting any task", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      error: "forbidden",
    }), { status: 403, headers: { "Content-Type": "application/json" } }));
    vi.stubGlobal("fetch", fetchMock);
    setApiInstance(new ApiClient("https://api.example.test"));
    const queryClient = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
    const { result } = renderHook(
      () => useSteerAgentThread("workspace-1", "opener-task"),
      { wrapper: wrapper(queryClient) },
    );

    await expect(act(async () => result.current.mutateAsync("queued-task")))
      .rejects.toMatchObject({ status: 403 });
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("rejects when interrupting the selected active task fails", async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({
        task_id: "queued-task",
        active_task_id: "running-task",
      }), { status: 200, headers: { "Content-Type": "application/json" } }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ error: "already finished" }), {
        status: 409,
        headers: { "Content-Type": "application/json" },
      }));
    vi.stubGlobal("fetch", fetchMock);
    setApiInstance(new ApiClient("https://api.example.test"));
    const queryClient = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
    const { result } = renderHook(
      () => useSteerAgentThread("workspace-1", "opener-task"),
      { wrapper: wrapper(queryClient) },
    );

    await expect(act(async () => result.current.mutateAsync("queued-task")))
      .rejects.toMatchObject({ status: 409 });
  });
});
