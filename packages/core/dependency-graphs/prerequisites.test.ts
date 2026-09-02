// @vitest-environment node
import { describe, expect, it } from "vitest";
import type { DependencyGraphResponse } from "../types";
import { selectDependencyPrerequisiteState } from "./prerequisites";

const graph = {
  plan: { id: "plan-1", attention_required: false, attention_reason: null },
  nodes: [
    { issue_id: "source-done", title: "A completed prerequisite", blocked_by: [] },
    { issue_id: "source-open", title: "B open prerequisite", blocked_by: [] },
    {
      issue_id: "target",
      title: "Target issue",
      ready: false,
      blocked_by: ["source-open"],
    },
  ],
  edges: [
    {
      id: "edge-done",
      type: "hard",
      from_issue_id: "source-done",
      to_issue_id: "target",
      satisfied: false,
    },
    {
      id: "edge-open",
      type: "hard",
      from_issue_id: "source-open",
      to_issue_id: "target",
      satisfied: true,
    },
    {
      id: "edge-soft",
      type: "soft",
      from_issue_id: "source-done",
      to_issue_id: "target",
    },
  ],
} as unknown as DependencyGraphResponse;

describe("selectDependencyPrerequisiteState", () => {
  it("uses the Go blocked_by gate instead of compatibility edge fields", () => {
    const state = selectDependencyPrerequisiteState(graph, "target");

    expect(state).toMatchObject({
      blockedBy: ["source-open"],
      satisfied: 1,
      total: 2,
      ready: false,
    });
    expect(state.prerequisites.map((item) => [item.node.issue_id, item.satisfied])).toEqual([
      ["source-done", true],
      ["source-open", false],
    ]);
  });
});
