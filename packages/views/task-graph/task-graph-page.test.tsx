// @vitest-environment node
import { describe, expect, it } from "vitest";
import type { DependencyGraphResponse } from "@patchbay/core/types";
import {
  edgeEndpoint,
  nodeMatchesFilter,
  summarizeGraphs,
} from "./graph-utils";

const graph = {
  plan: {
    id: "plan-1",
    status: "active",
  },
  readiness: { total: 0, ready: 0, running: 0, blocked: 0, done: 0, cancelled: 0 },
  nodes: [
    { id: "node-1", issue_id: "issue-1", status: "ready", readiness: { state: "ready" } },
    { id: "node-2", issue_id: "issue-2", status: "blocked", readiness: { state: "blocked" } },
  ],
  edges: [],
} as unknown as DependencyGraphResponse;

describe("task graph presentation helpers", () => {
  it("summarizes persisted node readiness for the workspace header", () => {
    expect(summarizeGraphs([graph])).toEqual({
      total: 2,
      ready: 1,
      running: 0,
      blocked: 1,
    });
  });

  it("filters by readiness state and preserves unknown states for all", () => {
    expect(nodeMatchesFilter(graph.nodes[0]!, "ready")).toBe(true);
    expect(nodeMatchesFilter(graph.nodes[0]!, "blocked")).toBe(false);
    expect(nodeMatchesFilter(graph.nodes[0]!, "all")).toBe(true);
  });

  it("falls back to issue ids when enriched edge endpoints are absent", () => {
    expect(
      edgeEndpoint(
        {
          from: "",
          to: "temp-2",
          from_issue_id: "issue-1",
          to_issue_id: "issue-2",
        } as never,
        "from",
      ),
    ).toBe("issue-1");
    expect(
      edgeEndpoint(
        {
          from: "temp-1",
          to: "",
          from_issue_id: "issue-1",
          to_issue_id: "issue-2",
        } as never,
        "to",
      ),
    ).toBe("issue-2");
  });
});
