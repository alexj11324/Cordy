// @vitest-environment node
import { describe, expect, it } from "vitest";
import type { DependencyGraphResponse } from "@patchbay/core/types";
import {
  CANVAS_PADDING,
  COLUMN_GAP,
  NODE_HEIGHT,
  NODE_WIDTH,
  ROW_GAP,
  layoutGraph,
} from "./graph-layout";

function makeGraph(
  nodes: unknown[],
  edges: unknown[] = [],
): DependencyGraphResponse {
  return {
    plan: { id: "plan-1", status: "active" },
    readiness: { total: 0, ready: 0, running: 0, blocked: 0, done: 0, cancelled: 0 },
    nodes,
    edges,
  } as unknown as DependencyGraphResponse;
}

const graph = makeGraph(
  [
    {
      id: "n1",
      temp_id: "t1",
      issue_id: "i1",
      title: "Alpha",
      wave: 1,
      status: "ready",
      readiness: { state: "ready" },
    },
    {
      id: "n2",
      temp_id: "t2",
      issue_id: "i2",
      title: "Bravo",
      wave: 2,
      status: "blocked",
      readiness: { state: "blocked" },
    },
    {
      id: "n3",
      temp_id: "t3",
      issue_id: "i3",
      title: "Charlie",
      wave: 2,
      status: "ready",
      readiness: { state: "ready" },
    },
  ],
  [
    { id: "e1", from: "t1", to: "t2", from_issue_id: "i1", to_issue_id: "i2" },
    { id: "e2", from: "t1", to: "t3", from_issue_id: "i1", to_issue_id: "i3" },
  ],
);

describe("dependency graph layout", () => {
  it("puts each wave in its own column, in execution order", () => {
    const layout = layoutGraph(graph, "all");
    expect(layout.columns).toEqual([
      { wave: 1, x: CANVAS_PADDING },
      { wave: 2, x: CANVAS_PADDING + NODE_WIDTH + COLUMN_GAP },
    ]);
    const alpha = layout.nodes.find((n) => n.id === "n1")!;
    const bravo = layout.nodes.find((n) => n.id === "n2")!;
    expect(alpha.x).toBeLessThan(bravo.x);
  });

  it("stacks tasks within a wave without overlapping them", () => {
    const layout = layoutGraph(graph, "all");
    const waveTwo = layout.nodes
      .filter((n) => n.wave === 2)
      .sort((a, b) => a.y - b.y);
    expect(waveTwo).toHaveLength(2);
    expect(waveTwo[0]!.y).toBe(CANVAS_PADDING);
    expect(waveTwo[1]!.y).toBe(CANVAS_PADDING + NODE_HEIGHT + ROW_GAP);
    expect(waveTwo[1]!.y - waveTwo[0]!.y).toBeGreaterThanOrEqual(NODE_HEIGHT);
    // Same column, so they must not share a row.
    expect(waveTwo[0]!.x).toBe(waveTwo[1]!.x);
  });

  it("draws every edge whose endpoints are both on the canvas", () => {
    const layout = layoutGraph(graph, "all");
    expect(layout.edges.map((e) => e.id).sort()).toEqual(["e1", "e2"]);
    expect(layout.undrawnEdgeCount).toBe(0);
    for (const edge of layout.edges) {
      expect(edge.path).toMatch(/^M [\d.]+ [\d.]+ C /);
    }
  });

  it("resolves edge endpoints by issue id when temp ids are absent", () => {
    const byIssueId = makeGraph(graph.nodes, [
      { id: "e1", from: "", to: "", from_issue_id: "i1", to_issue_id: "i2" },
    ]);
    const layout = layoutGraph(byIssueId, "all");
    expect(layout.edges).toHaveLength(1);
    expect(layout.edges[0]!.fromId).toBe("n1");
    expect(layout.edges[0]!.toId).toBe("n2");
  });

  it("reports edges it cannot draw instead of dropping them silently", () => {
    // Filtering to `ready` removes Bravo, so the edge into it has nowhere to
    // land. The count is what the canvas shows the user.
    const layout = layoutGraph(graph, "ready");
    expect(layout.nodes.map((n) => n.id).sort()).toEqual(["n1", "n3"]);
    expect(layout.edges.map((e) => e.id)).toEqual(["e2"]);
    expect(layout.undrawnEdgeCount).toBe(1);
  });

  it("does not draw an edge from a node to itself", () => {
    const selfEdge = makeGraph(graph.nodes, [
      { id: "e-self", from: "t1", to: "i1", from_issue_id: "i1", to_issue_id: "i1" },
    ]);
    const layout = layoutGraph(selfEdge, "all");
    expect(layout.edges).toHaveLength(0);
    expect(layout.undrawnEdgeCount).toBe(1);
  });

  it("sizes the canvas to contain every node plus padding", () => {
    const layout = layoutGraph(graph, "all");
    for (const node of layout.nodes) {
      expect(node.x + node.width).toBeLessThanOrEqual(layout.width);
      expect(node.y + node.height).toBeLessThanOrEqual(layout.height);
    }
    expect(layout.width).toBe(
      CANVAS_PADDING + NODE_WIDTH + COLUMN_GAP + NODE_WIDTH + CANVAS_PADDING,
    );
  });

  it("returns an empty, padded canvas when the filter matches nothing", () => {
    const layout = layoutGraph(graph, "running");
    expect(layout.nodes).toEqual([]);
    expect(layout.edges).toEqual([]);
    expect(layout.columns).toEqual([]);
    expect(layout.undrawnEdgeCount).toBe(2);
    expect(layout.width).toBe(CANVAS_PADDING * 2);
  });

  it("orders nodes in a wave deterministically", () => {
    const reversed = makeGraph([...graph.nodes].reverse(), graph.edges);
    const first = layoutGraph(graph, "all").nodes.map((n) => n.id);
    const second = layoutGraph(reversed, "all").nodes.map((n) => n.id);
    expect(second).toEqual(first);
  });
});
