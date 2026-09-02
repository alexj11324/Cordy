import type {
  DependencyGraphEdge,
  DependencyGraphNode,
  DependencyGraphResponse,
} from "../types";

export type DependencyPrerequisite = {
  edge: DependencyGraphEdge;
  node: DependencyGraphNode;
  satisfied: boolean;
};

export type DependencyPrerequisiteState = {
  prerequisites: DependencyPrerequisite[];
  blockedBy: string[];
  satisfied: number;
  total: number;
  ready: boolean;
};

/**
 * Projects the persisted scheduler gate for one issue from the Go API shape.
 * `blocked_by` is authoritative; edge compatibility fields are intentionally
 * ignored because the detail endpoint does not return them.
 */
export function selectDependencyPrerequisiteState(
  graph: DependencyGraphResponse,
  issueId: string,
): DependencyPrerequisiteState {
  const target = graph.nodes.find((node) => node.issue_id === issueId);
  const blockedBy = target?.blocked_by ?? [];
  const blockedSet = new Set(blockedBy);
  const nodesByIssueId = new Map(
    graph.nodes.map((node) => [node.issue_id, node] as const),
  );
  const prerequisites = graph.edges
    .filter((edge) => edge.type === "hard" && edge.to_issue_id === issueId)
    .flatMap((edge) => {
      const node = nodesByIssueId.get(edge.from_issue_id);
      return node
        ? [{ edge, node, satisfied: !blockedSet.has(edge.from_issue_id) }]
        : [];
    })
    .sort((left, right) => left.node.title.localeCompare(right.node.title));

  return {
    prerequisites,
    blockedBy,
    satisfied: prerequisites.filter((item) => item.satisfied).length,
    total: prerequisites.length,
    ready: target?.ready ?? false,
  };
}
