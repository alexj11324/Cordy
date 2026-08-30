import { describe, expect, it } from "vitest";
import { AgentThreadResponseSchema } from "./schemas";

describe("mobile Agent thread availability compatibility", () => {
  it("keeps history when a newer availability state is received", () => {
    const parsed = AgentThreadResponseSchema.safeParse({
      task: { id: "task-1", status: "completed" },
      thread_tasks: [{ id: "task-1", status: "completed" }],
      current_task_id: "task-1",
      agent: { id: "agent-1", name: "Builder" },
      events: [{ task_id: "task-1", seq: 1, type: "tool_use", content: "done" }],
      availability: {
        state: "provider_reconnecting",
        reason_code: "provider_reconnecting",
      },
      can_continue: true,
    });

    expect(parsed.success).toBe(true);
    if (!parsed.success) return;
    expect(parsed.data.availability.state).toBe("unavailable");
    expect(parsed.data.events).toHaveLength(1);
    expect(parsed.data.thread_tasks[0]?.id).toBe("task-1");
  });
});
