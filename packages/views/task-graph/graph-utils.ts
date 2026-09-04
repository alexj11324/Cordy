import type {
  DependencyGraphEdge,
  DependencyGraphNode,
  DependencyGraphResponse,
} from "@patchbay/core/types";

export type GraphFilter = "all" | "ready" | "running" | "blocked";

type GraphSummary = {
  total: number;
  ready: number;
  running: number;
  blocked: number;
};

const EMPTY_SUMMARY: GraphSummary = {
  total: 0,
  ready: 0,
  running: 0,
  blocked: 0,
};

export function nodeMatchesFilter(
  node: DependencyGraphNode,
  filter: GraphFilter,
): boolean {
  if (filter === "all") return true;
  return (node.readiness?.state ?? node.status) === filter;
}

export function edgeEndpoint(
  edge: DependencyGraphEdge,
  side: "from" | "to",
): string {
  return side === "from"
    ? edge.from || edge.from_issue_id
    : edge.to || edge.to_issue_id;
}

export function summarizeGraphs(
  graphs: DependencyGraphResponse[],
): GraphSummary {
  return graphs.reduce((summary, graph) => {
    if (graph.nodes.length === 0) {
      return {
        total: summary.total + graph.readiness.total,
        ready: summary.ready + graph.readiness.ready,
        running: summary.running + graph.readiness.running,
        blocked: summary.blocked + graph.readiness.blocked,
      };
    }

    return graph.nodes.reduce((next, node) => {
      const state = node.readiness?.state ?? node.status;
      return {
        total: next.total + 1,
        ready: next.ready + (state === "ready" ? 1 : 0),
        running: next.running + (state === "running" ? 1 : 0),
        blocked: next.blocked + (state === "blocked" ? 1 : 0),
      };
    }, summary);
  }, EMPTY_SUMMARY);
}
