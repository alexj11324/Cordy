import { describe, expect, it } from "vitest";
import { foldToolLifecycles, taskMessageState } from "./chat-tool-state";

describe("mobile task tool state", () => {
  it("keeps every AI Elements state supplied by the server", () => {
    expect(
      taskMessageState({ type: "tool_use", state: "input-streaming" }),
    ).toBe("input-streaming");
    expect(
      taskMessageState({ type: "tool_result", state: "output-denied" }),
    ).toBe("output-denied");
  });

  it("uses lifecycle defaults for legacy rows without state", () => {
    expect(taskMessageState({ type: "tool_use" })).toBe("input-available");
    expect(taskMessageState({ type: "tool_result" })).toBe("output-available");
  });

  it("folds a matching result into one terminal tool row", () => {
    const rows = foldToolLifecycles([
      {
        task_id: "task-1",
        issue_id: "",
        seq: 1,
        type: "tool_use",
        call_id: "call-1",
        tool: "read",
        input: { path: "a" },
        state: "input-available",
      },
      {
        task_id: "task-1",
        issue_id: "",
        seq: 2,
        type: "tool_result",
        call_id: "call-1",
        output: "done",
        state: "output-available",
      },
    ]);
    expect(rows).toEqual([
      expect.objectContaining({
        type: "tool_use",
        call_id: "call-1",
        input: { path: "a" },
        output: "done",
        state: "output-available",
      }),
    ]);
  });
});
