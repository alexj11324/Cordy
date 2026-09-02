import { describe, expect, it } from "vitest";
import type { AgentTask } from "../types";
import { deriveAgentThreadTaskState, isAgentTaskActive } from "./task-state";

const task = (id: string, status: AgentTask["status"]): AgentTask => ({
  id,
  agent_id: "agent",
  runtime_id: "runtime",
  issue_id: "issue",
  status,
  priority: 1,
  dispatched_at: null,
  started_at: null,
  completed_at: null,
  result: null,
  error: null,
  created_at: `2026-01-01T00:00:0${id}.000Z`,
});

describe("agent thread task state", () => {
  it("keeps the executing run as head and later turns queued", () => {
    const state = deriveAgentThreadTaskState([
      task("1", "running"),
      task("2", "queued"),
    ]);
    expect(state.pendingTask?.task_id).toBe("1");
    expect(state.queuedTasks.map((item) => item.task_id)).toEqual(["2"]);
  });

  it("treats a deferred continuation as active", () => {
    const deferred = task("1", "deferred");
    expect(isAgentTaskActive(deferred)).toBe(true);
    expect(deriveAgentThreadTaskState([deferred]).pendingTask?.task_id).toBe("1");
  });

  it("fails closed for unknown statuses", () => {
    expect(isAgentTaskActive(task("1", "future" as AgentTask["status"]))).toBe(false);
  });
});
