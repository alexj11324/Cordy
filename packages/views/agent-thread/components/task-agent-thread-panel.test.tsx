// @vitest-environment jsdom

import { act, render } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  steer: vi.fn(),
  toastError: vi.fn(),
  surfaceProps: undefined as Record<string, unknown> | undefined,
}));

vi.mock("@tanstack/react-query", () => ({
  useQueryClient: () => ({ setQueryData: vi.fn() }),
  useQuery: () => ({
    isPending: false,
    data: {
      current_task_id: "queued-task",
      can_continue: true,
      availability: { state: "available" },
      agent: { id: "agent-1", name: "Agent One", avatar_url: null },
      events: [],
      task: {
        id: "queued-task",
        agent_id: "agent-1",
        runtime_id: "runtime-1",
        issue_id: "issue-1",
        status: "queued",
        priority: 1,
        dispatched_at: null,
        started_at: null,
        completed_at: null,
        result: null,
        error: null,
        created_at: "2026-09-06T12:01:00Z",
      },
      thread_tasks: [{
        id: "running-task",
        agent_id: "agent-1",
        runtime_id: "runtime-1",
        issue_id: "issue-1",
        status: "running",
        priority: 1,
        dispatched_at: "2026-09-06T12:00:00Z",
        started_at: "2026-09-06T12:00:01Z",
        completed_at: null,
        result: null,
        error: null,
        created_at: "2026-09-06T12:00:00Z",
      }, {
        id: "queued-task",
        agent_id: "agent-1",
        runtime_id: "runtime-1",
        issue_id: "issue-1",
        status: "queued",
        priority: 1,
        dispatched_at: null,
        started_at: null,
        completed_at: null,
        result: null,
        error: null,
        created_at: "2026-09-06T12:01:00Z",
      }],
    },
  }),
}));

vi.mock("@orvilo/core/agent-thread", async (importOriginal) => ({
  ...await importOriginal<typeof import("@orvilo/core/agent-thread")>(),
  agentThreadOptions: () => ({}),
  useContinueAgentThread: () => ({ mutateAsync: vi.fn() }),
  useSteerAgentThread: () => ({ mutateAsync: mocks.steer }),
}));

vi.mock("sonner", () => ({ toast: { error: mocks.toastError } }));
vi.mock("../../i18n", () => ({ useT: () => ({ t: () => "translated" }) }));
vi.mock("./agent-thread-surface", () => ({
  AgentThreadSurface: (props: Record<string, unknown>) => {
    mocks.surfaceProps = props;
    return <div data-testid="agent-thread-surface" />;
  },
}));

import { TaskAgentThreadPanel } from "./task-agent-thread-panel";

describe("TaskAgentThreadPanel queue actions", () => {
  beforeEach(() => {
    mocks.steer.mockReset();
    mocks.toastError.mockReset();
    mocks.surfaceProps = undefined;
  });

  it("wires the selected queued task to task-thread Steer", async () => {
    mocks.steer.mockResolvedValue({
      task_id: "queued-task",
      active_task_id: "running-task",
    });
    render(
      <TaskAgentThreadPanel
        workspaceId="workspace-1"
        taskId="opener-task"
        onClose={() => {}}
      />,
    );

    const steer = mocks.surfaceProps?.onSendQueuedTaskNow;
    expect(steer).toBeTypeOf("function");
    await act(async () => {
      await (steer as (taskId: string) => Promise<void>)("queued-task");
    });

    expect(mocks.steer).toHaveBeenCalledWith("queued-task");
    expect(mocks.toastError).not.toHaveBeenCalled();
  });

  it("shows an error when Steer fails", async () => {
    mocks.steer.mockRejectedValue(new Error("steer failed"));
    render(
      <TaskAgentThreadPanel
        workspaceId="workspace-1"
        taskId="opener-task"
        onClose={() => {}}
      />,
    );

    await act(async () => {
      await (mocks.surfaceProps?.onSendQueuedTaskNow as (taskId: string) => Promise<void>)("queued-task");
    });

    expect(mocks.toastError).toHaveBeenCalledWith("translated");
  });
});
