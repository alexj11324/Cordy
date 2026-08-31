import { describe, expect, it } from "vitest";
import type { AgentTask } from "@patchbay/core/types";
import { deriveAgentThreadTaskState } from "@patchbay/core/agent-thread";
import {
  buildTaskAgentThreadMessages,
  pendingTaskForAgentThread,
} from "./task-agent-thread";

function task(overrides: Partial<AgentTask> = {}): AgentTask {
  return {
    id: "task-1",
    agent_id: "agent-1",
    runtime_id: "runtime-1",
    issue_id: "",
    status: "completed",
    priority: 0,
    dispatched_at: null,
    started_at: "2026-08-29T12:00:00Z",
    completed_at: "2026-08-29T12:01:00Z",
    result: { output: "done" },
    error: null,
    created_at: "2026-08-29T11:59:00Z",
    ...overrides,
  };
}

describe("task Agent thread projection", () => {
  it("keeps terminal task events on the assistant turn", () => {
    const messages = buildTaskAgentThreadMessages(
      task(),
      "Continue this thread.",
    );

    expect(messages).toHaveLength(2);
    expect(messages[0]?.role).toBe("user");
    expect(messages[1]).toMatchObject({
      role: "assistant",
      task_id: "task-1",
      content: "done",
    });
    expect(pendingTaskForAgentThread(task())).toBeNull();
  });

  it("renders a live task as a pending shared-thread turn", () => {
    const live = task({ status: "running", completed_at: null, result: null });
    const messages = buildTaskAgentThreadMessages(
      live,
      "Continue this thread.",
    );

    expect(messages).toHaveLength(1);
    expect(pendingTaskForAgentThread(live)).toMatchObject({
      task_id: "task-1",
      status: "running",
    });
  });

  it("projects each continuation as its own interactive thread turn", () => {
    const continuation = task({
      id: "task-2",
      parent_task_id: "task-1",
      trigger_summary: "Please inspect the failing assertion.",
      created_at: "2026-08-29T12:02:00Z",
      started_at: "2026-08-29T12:02:00Z",
      completed_at: "2026-08-29T12:03:00Z",
      result: { output: "The assertion is fixed." },
    });
    const messages = [task(), continuation].flatMap((threadTask) =>
      buildTaskAgentThreadMessages(threadTask, "Continue this thread."),
    );

    expect(messages.map((message) => message.role)).toEqual([
      "user",
      "assistant",
      "user",
      "assistant",
    ]);
    expect(messages[2]).toMatchObject({
      id: "task-prompt:task-2",
      content: "Please inspect the failing assertion.",
    });
    expect(messages[3]).toMatchObject({
      task_id: "task-2",
      content: "The assertion is fixed.",
    });
  });

  it("renders the complete continuation turn instead of its bounded summary", () => {
    const continuation = task({
      id: "task-2",
      parent_task_id: "task-1",
      trigger_summary: "Bounded summary",
      agent_thread_message:
        "The complete continuation instruction with details beyond the summary limit.",
      created_at: "2026-08-29T12:02:00Z",
      started_at: "2026-08-29T12:02:00Z",
      completed_at: "2026-08-29T12:03:00Z",
      result: { output: "done" },
    });

    const messages = buildTaskAgentThreadMessages(
      continuation,
      "Continue this thread.",
    );

    expect(messages[0]).toMatchObject({
      role: "user",
      content:
        "The complete continuation instruction with details beyond the summary limit.",
    });
  });

  it("keeps a running parent as the lane head when a newer continuation is queued", () => {
    const running = task({
      status: "running",
      completed_at: null,
      result: null,
    });
    const queued = task({
      id: "task-2",
      status: "queued",
      parent_task_id: "task-1",
      trigger_summary: "Inspect the next failure",
      created_at: "2026-08-29T12:02:00Z",
      started_at: null,
      completed_at: null,
      result: null,
    });

    const state = deriveAgentThreadTaskState([running, queued]);

    expect(state.headTask?.id).toBe("task-1");
    expect(state.executingTask?.id).toBe("task-1");
    expect(state.pendingTask?.task_id).toBe("task-1");
    expect(state.queuedTasks).toEqual([
      expect.objectContaining({
        task_id: "task-2",
        status: "queued",
        content: "Inspect the next failure",
      }),
    ]);
  });

  it("keeps a deferred continuation behind the executing parent", () => {
    const running = task({
      status: "running",
      completed_at: null,
      result: null,
    });
    const deferred = task({
      id: "task-2",
      status: "deferred",
      parent_task_id: "task-1",
      trigger_summary: "Continue after the current turn",
      created_at: "2026-08-29T12:02:00Z",
      started_at: null,
      completed_at: null,
      result: null,
    });

    const state = deriveAgentThreadTaskState([running, deferred]);

    expect(state.executingTask?.id).toBe("task-1");
    expect(state.pendingTask?.task_id).toBe("task-1");
    expect(state.queuedTasks).toEqual([
      expect.objectContaining({
        task_id: "task-2",
        status: "deferred",
        content: "Continue after the current turn",
      }),
    ]);
  });

  it("treats unknown provider states as terminal rather than active", () => {
    const futureTask = task({ status: "provider_paused" });

    expect(deriveAgentThreadTaskState([futureTask]).pendingTask).toBeNull();
    expect(
      buildTaskAgentThreadMessages(futureTask, "Continue this thread."),
    ).toHaveLength(2);
  });
});
