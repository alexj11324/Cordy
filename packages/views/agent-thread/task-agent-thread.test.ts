import { describe, expect, it } from "vitest";
import type { AgentTask } from "@patchbay/core/types";
import { buildTaskAgentThreadMessages } from "./task-agent-thread";

function task(overrides: Partial<AgentTask> = {}): AgentTask {
  return {
    id: "task-1",
    agent_id: "agent-1",
    runtime_id: "runtime-1",
    issue_id: "issue-1",
    status: "completed",
    priority: 2,
    dispatched_at: null,
    started_at: "2026-01-01T00:00:01Z",
    completed_at: "2026-01-01T00:00:02Z",
    result: { output: "done" },
    error: null,
    created_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

describe("buildTaskAgentThreadMessages", () => {
  it("keeps the full continuation turn and links the run transcript", () => {
    const messages = buildTaskAgentThreadMessages(task({
      agent_thread_message: "the complete next turn",
      trigger_summary: "truncated next turn",
    }), "fallback");
    expect(messages.map((message) => message.content)).toEqual([
      "the complete next turn",
      "done",
    ]);
    expect(messages[1]?.task_id).toBe("task-1");
  });

  it("renders an active continuation as a pending user turn", () => {
    const messages = buildTaskAgentThreadMessages(task({
      status: "running",
      completed_at: null,
      result: null,
      agent_thread_message: "keep going",
    }), "fallback");
    expect(messages).toHaveLength(1);
    expect(messages[0]?.role).toBe("user");
  });
});
