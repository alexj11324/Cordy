"use client";

/**
 * The interactive dependency-graph canvas.
 *
 * This is a renderer: every coordinate comes from `layoutGraph`, so the only
 * thing here is SVG and interaction. Nodes are real focusable controls with
 * `aria-label`s rather than decorative shapes, and activating one selects it
 * in the same inspector the list view drives, so keyboard and pointer users
 * reach the same state.
 *
 * The canvas is never the only way to read a plan — the page keeps the
 * wave/edge list one control away and that list stays the accessible path for
 * anyone a node-link diagram does not serve.
 */
import type { DependencyGraphNode, DependencyGraphResponse } from "@patchbay/core/types";
import { cn } from "@patchbay/ui/lib/utils";
import { layoutGraph, type LaidOutNode } from "./graph-layout";
import type { GraphFilter } from "./graph-utils";

export type CanvasSelection =
  | { kind: "node"; planId: string; nodeId: string }
  | { kind: "edge"; planId: string; edgeId: string }
  | null;

function nodeIdentifier(node: DependencyGraphNode): string {
  return node.issue?.identifier || node.issue_id || node.temp_id || node.id;
}

function nodeTitle(node: DependencyGraphNode): string {
  return node.title.trim() || node.issue?.title || nodeIdentifier(node);
}

function nodeState(node: DependencyGraphNode): string {
  return node.readiness?.state || node.status || "todo";
}

/**
 * Fill and stroke per readiness state. State is also carried in each node's
 * `aria-label`, so colour is never the only signal.
 */
function nodeTone(state: string): { fill: string; stroke: string } {
  switch (state) {
    case "ready":
      return { fill: "fill-emerald-500/10", stroke: "stroke-emerald-500/50" };
    case "running":
      return { fill: "fill-blue-500/10", stroke: "stroke-blue-500/50" };
    case "blocked":
      return { fill: "fill-amber-500/10", stroke: "stroke-amber-500/50" };
    case "done":
      return { fill: "fill-muted", stroke: "stroke-border" };
    default:
      return { fill: "fill-background", stroke: "stroke-border" };
  }
}

function truncate(text: string, max: number): string {
  return text.length <= max ? text : `${text.slice(0, max - 1)}…`;
}

export function GraphCanvas({
  graph,
  filter,
  selection,
  onSelect,
  labels,
}: {
  graph: DependencyGraphResponse;
  filter: GraphFilter;
  selection: CanvasSelection;
  onSelect: (selection: CanvasSelection) => void;
  labels: {
    canvas: string;
    nodeHint: (args: { identifier: string; title: string; state: string; wave: number }) => string;
    edgeHint: (args: { from: string; to: string; satisfied: boolean }) => string;
    waveColumn: (wave: number) => string;
    empty: string;
    undrawn: (count: number) => string;
  };
}) {
  const layout = layoutGraph(graph, filter);
  const planId = graph.plan.id;

  if (layout.nodes.length === 0) {
    return (
      <p className="py-8 text-center text-caption text-muted-foreground">
        {labels.empty}
      </p>
    );
  }

  const selectNode = (node: LaidOutNode) =>
    onSelect({ kind: "node", planId, nodeId: node.id });

  return (
    <div className="mt-4">
      {/* The canvas scrolls inside its own box; the page must never scroll
          sideways because a plan is wide. */}
      <div className="overflow-x-auto rounded-lg bg-muted/20">
        <svg
          role="group"
          aria-label={labels.canvas}
          width={layout.width}
          height={layout.height}
          viewBox={`0 0 ${layout.width} ${layout.height}`}
          className="max-w-none"
        >
          <defs>
            <marker
              id={`dependency-arrow-${planId}`}
              markerWidth="8"
              markerHeight="8"
              refX="7"
              refY="4"
              orient="auto"
            >
              <path d="M 0 0 L 8 4 L 0 8 z" className="fill-muted-foreground/60" />
            </marker>
          </defs>

          {layout.columns.map((column) => (
            <text
              key={column.wave}
              x={column.x}
              y={12}
              className="fill-muted-foreground text-[10px]"
            >
              {labels.waveColumn(column.wave)}
            </text>
          ))}

          {layout.edges.map((laidOutEdge) => {
            const selected =
              selection?.kind === "edge" &&
              selection.planId === planId &&
              selection.edgeId === laidOutEdge.id;
            const from = layout.nodes.find((n) => n.id === laidOutEdge.fromId);
            const to = layout.nodes.find((n) => n.id === laidOutEdge.toId);
            return (
              <g key={laidOutEdge.id}>
                <path
                  d={laidOutEdge.path}
                  fill="none"
                  markerEnd={`url(#dependency-arrow-${planId})`}
                  strokeWidth={selected ? 2.5 : 1.5}
                  strokeDasharray={laidOutEdge.edge.satisfied ? undefined : "5 4"}
                  className={cn(
                    selected
                      ? "stroke-brand"
                      : laidOutEdge.edge.satisfied
                        ? "stroke-emerald-500/60"
                        : "stroke-amber-500/60",
                  )}
                />
                {/* A wide transparent path over the visible one so the edge is
                    clickable without demanding pixel-perfect aim. */}
                <path
                  d={laidOutEdge.path}
                  fill="none"
                  strokeWidth={12}
                  stroke="transparent"
                  className="cursor-pointer"
                  role="button"
                  tabIndex={0}
                  aria-pressed={selected}
                  aria-label={labels.edgeHint({
                    from: from ? nodeIdentifier(from.node) : laidOutEdge.fromId,
                    to: to ? nodeIdentifier(to.node) : laidOutEdge.toId,
                    satisfied: laidOutEdge.edge.satisfied,
                  })}
                  onClick={() =>
                    onSelect({ kind: "edge", planId, edgeId: laidOutEdge.id })
                  }
                  onKeyDown={(event) => {
                    if (event.key !== "Enter" && event.key !== " ") return;
                    event.preventDefault();
                    onSelect({ kind: "edge", planId, edgeId: laidOutEdge.id });
                  }}
                />
              </g>
            );
          })}

          {layout.nodes.map((laidOut) => {
            const state = nodeState(laidOut.node);
            const tone = nodeTone(state);
            const identifier = nodeIdentifier(laidOut.node);
            const title = nodeTitle(laidOut.node);
            const selected =
              selection?.kind === "node" &&
              selection.planId === planId &&
              selection.nodeId === laidOut.id;
            return (
              <g
                key={laidOut.id}
                role="button"
                tabIndex={0}
                aria-pressed={selected}
                aria-label={labels.nodeHint({
                  identifier,
                  title,
                  state,
                  wave: laidOut.wave,
                })}
                data-testid="dependency-graph-canvas-node"
                className="cursor-pointer focus:outline-none"
                onClick={() => selectNode(laidOut)}
                onKeyDown={(event) => {
                  if (event.key !== "Enter" && event.key !== " ") return;
                  event.preventDefault();
                  selectNode(laidOut);
                }}
              >
                <rect
                  x={laidOut.x}
                  y={laidOut.y}
                  width={laidOut.width}
                  height={laidOut.height}
                  rx={8}
                  strokeWidth={selected ? 2 : 1}
                  className={cn(
                    tone.fill,
                    selected ? "stroke-brand" : tone.stroke,
                  )}
                />
                <text
                  x={laidOut.x + 10}
                  y={laidOut.y + 22}
                  className="fill-primary text-[11px] font-medium"
                >
                  {truncate(identifier, 20)}
                </text>
                <text
                  x={laidOut.x + 10}
                  y={laidOut.y + 40}
                  className="fill-foreground text-[11px]"
                >
                  {truncate(title, 22)}
                </text>
                <text
                  x={laidOut.x + 10}
                  y={laidOut.y + 55}
                  className="fill-muted-foreground text-[10px]"
                >
                  {truncate(state, 22)}
                </text>
              </g>
            );
          })}
        </svg>
      </div>
      {layout.undrawnEdgeCount > 0 ? (
        <p className="mt-2 text-caption text-muted-foreground">
          {labels.undrawn(layout.undrawnEdgeCount)}
        </p>
      ) : null}
    </div>
  );
}
