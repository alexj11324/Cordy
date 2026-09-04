/**
 * Pure layout for the dependency graph canvas.
 *
 * The canvas is a wave-ordered node-link diagram: waves are columns in
 * execution order, tasks stack down each column, and every edge is drawn from
 * its source node's right edge to its target node's left edge. Keeping the
 * geometry here — with no React, no DOM, and no SVG — is what makes the layout
 * testable, and the canvas component a renderer with no arithmetic of its own.
 *
 * Edges whose endpoints are not both present in the laid-out set (filtered
 * out, or referencing a node the payload never included) are reported
 * separately rather than dropped silently: the canvas tells the user how many
 * dependencies it could not draw, so a filtered view never looks like a graph
 * with fewer dependencies than it has.
 */
import type {
  DependencyGraphEdge,
  DependencyGraphNode,
  DependencyGraphResponse,
} from "@patchbay/core/types";
import { edgeEndpoint, nodeMatchesFilter, type GraphFilter } from "./graph-utils";

export const NODE_WIDTH = 176;
export const NODE_HEIGHT = 64;
export const COLUMN_GAP = 72;
export const ROW_GAP = 20;
export const CANVAS_PADDING = 24;

export type LaidOutNode = {
  node: DependencyGraphNode;
  id: string;
  wave: number;
  x: number;
  y: number;
  width: number;
  height: number;
};

export type LaidOutEdge = {
  edge: DependencyGraphEdge;
  id: string;
  fromId: string;
  toId: string;
  /** SVG cubic path from the source's right edge to the target's left edge. */
  path: string;
};

export type GraphLayout = {
  nodes: LaidOutNode[];
  edges: LaidOutEdge[];
  /** Waves in execution order, each with its column x offset. */
  columns: { wave: number; x: number }[];
  width: number;
  height: number;
  /** Edges that could not be drawn because an endpoint is not on the canvas. */
  undrawnEdgeCount: number;
};

export function graphNodeId(node: DependencyGraphNode): string {
  return node.id || node.temp_id || node.issue_id;
}

/**
 * Every identifier a payload might use to name this node in an edge endpoint.
 * Edges reference `temp_id` before an issue exists and `issue_id` afterwards,
 * and enriched payloads sometimes carry the issue's own id or identifier.
 */
function nodeAliases(node: DependencyGraphNode): string[] {
  return [
    node.id,
    node.temp_id,
    node.issue_id,
    node.issue?.id,
    node.issue?.identifier,
  ].filter((alias): alias is string => Boolean(alias));
}

function sortKey(node: DependencyGraphNode): string {
  return (
    node.issue?.identifier ||
    node.title.trim() ||
    node.issue_id ||
    node.temp_id ||
    node.id
  );
}

export function layoutGraph(
  graph: DependencyGraphResponse,
  filter: GraphFilter,
): GraphLayout {
  const visible = graph.nodes.filter((node) => nodeMatchesFilter(node, filter));

  const waves = Array.from(new Set(visible.map((node) => node.wave))).sort(
    (left, right) => left - right,
  );
  const columns = waves.map((wave, index) => ({
    wave,
    x: CANVAS_PADDING + index * (NODE_WIDTH + COLUMN_GAP),
  }));
  const columnX = new Map(columns.map((column) => [column.wave, column.x]));

  const nodes: LaidOutNode[] = [];
  for (const wave of waves) {
    const waveNodes = visible
      .filter((node) => node.wave === wave)
      .sort((left, right) => sortKey(left).localeCompare(sortKey(right)));
    waveNodes.forEach((node, row) => {
      nodes.push({
        node,
        id: graphNodeId(node),
        wave,
        x: columnX.get(wave) ?? CANVAS_PADDING,
        y: CANVAS_PADDING + row * (NODE_HEIGHT + ROW_GAP),
        width: NODE_WIDTH,
        height: NODE_HEIGHT,
      });
    });
  }

  // One alias table for the whole graph so an edge endpoint resolves in a
  // single lookup rather than a scan per edge.
  const byAlias = new Map<string, LaidOutNode>();
  for (const laidOut of nodes) {
    for (const alias of nodeAliases(laidOut.node)) {
      if (!byAlias.has(alias)) byAlias.set(alias, laidOut);
    }
  }

  const edges: LaidOutEdge[] = [];
  let undrawnEdgeCount = 0;
  for (const edge of graph.edges) {
    const from = byAlias.get(edgeEndpoint(edge, "from"));
    const to = byAlias.get(edgeEndpoint(edge, "to"));
    if (!from || !to || from === to) {
      undrawnEdgeCount += 1;
      continue;
    }
    edges.push({
      edge,
      id: edge.id,
      fromId: from.id,
      toId: to.id,
      path: edgePath(from, to),
    });
  }

  const maxRight = nodes.reduce(
    (widest, node) => Math.max(widest, node.x + node.width),
    CANVAS_PADDING,
  );
  const maxBottom = nodes.reduce(
    (tallest, node) => Math.max(tallest, node.y + node.height),
    CANVAS_PADDING,
  );

  return {
    nodes,
    edges,
    columns,
    width: maxRight + CANVAS_PADDING,
    height: maxBottom + CANVAS_PADDING,
    undrawnEdgeCount,
  };
}

/**
 * A horizontal cubic bezier. Control points sit halfway between the two nodes
 * so edges leave and enter horizontally and stay readable when a dependency
 * spans several waves.
 */
export function edgePath(from: LaidOutNode, to: LaidOutNode): string {
  const startX = from.x + from.width;
  const startY = from.y + from.height / 2;
  const endX = to.x;
  const endY = to.y + to.height / 2;
  const midX = startX + (endX - startX) / 2;
  return `M ${startX} ${startY} C ${midX} ${startY}, ${midX} ${endY}, ${endX} ${endY}`;
}
