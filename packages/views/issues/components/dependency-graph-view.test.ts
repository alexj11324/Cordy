import { describe, expect, it } from "vitest";
import type {
  DependencyGraphEdge,
  DependencyGraphNode,
  DependencyGraphResponse,
} from "@patchbay/core/types";
import { nodeMatchesFilter, relatedNodeKeys } from "./dependency-graph-view";

function node(tempId: string, state: string): DependencyGraphNode {
  return {
    temp_id: tempId,
    readiness: {
      state,
      gate_open: state === "ready",
      satisfied_prerequisites: 0,
      total_prerequisites: 0,
      unlock_condition: "",
    },
  } as DependencyGraphNode;
}

function graph(edges: Array<Pick<DependencyGraphEdge, "from" | "to">>): DependencyGraphResponse {
  return {
    plan: { id: "plan-1" },
    edges,
  } as DependencyGraphResponse;
}

describe("dependency graph selection model", () => {
  it("filters only by the persisted readiness state", () => {
    expect(nodeMatchesFilter(node("a", "ready"), "ready")).toBe(true);
    expect(nodeMatchesFilter(node("a", "blocked"), "ready")).toBe(false);
    expect(nodeMatchesFilter(node("a", "done"), "all")).toBe(true);
  });

  it("collects transitive upstream and downstream nodes for a selected node", () => {
    const relation = relatedNodeKeys(
      graph([
        { from: "a", to: "b" },
        { from: "b", to: "c" },
        { from: "d", to: "c" },
      ]),
      { planId: "plan-1", tempId: "b" },
    );

    expect(relation.upstream).toEqual(new Set(["a"]));
    expect(relation.downstream).toEqual(new Set(["c"]));
  });
});
