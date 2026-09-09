"use client";

/** Focusable task nodes and dependency edges share the selection inspector. */
import type {
  DependencyGraphNode,
  DependencyGraphResponse,
} from "@orvilo/core/types";
import { cn } from "@orvilo/ui/lib/utils";
import { useCallback, useLayoutEffect, useRef, useState } from "react";
import { BoardCardContent } from "../issues/components/board-card";
import { layoutGraph, type LaidOutNode } from "./graph-layout";

function GraphCard({ node, selected, onHeight }: {
  node: DependencyGraphNode;
  selected: boolean;
  onHeight: (id: string, height: number) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    const measure = () => onHeight(node.id, Math.ceil(element.getBoundingClientRect().height));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [node.id, onHeight]);
  return <div ref={ref} className={cn("group/card rounded-xl group-focus-visible/graph-node:outline-2 group-focus-visible/graph-node:outline-foreground", selected && "outline-2 outline-foreground")}>
    <BoardCardContent issue={node.issue} />
  </div>;
}

export type CanvasSelection =
  | { kind: "node"; planId: string; nodeId: string }
  | { kind: "edge"; planId: string; edgeId: string }
  | null;

function nodeIdentifier(node: DependencyGraphNode): string {
  return node.issue?.identifier || node.issue_id || node.temp_id || node.id;
}

function nodeTitle(node: DependencyGraphNode): string {
  return node.issue?.title || node.title.trim() || nodeIdentifier(node);
}

export function GraphCanvas({
  graph,
  selection,
  onSelect,
  labels,
}: {
  graph: DependencyGraphResponse;
  selection: CanvasSelection;
  onSelect: (selection: CanvasSelection) => void;
  labels: {
    canvas: string;
    nodeHint: (args: {
      identifier: string;
      title: string;
      state: string;
      wave: number;
    }) => string;
    edgeHint: (args: {
      from: string;
      to: string;
      satisfied: boolean;
    }) => string;
    status: (node: DependencyGraphNode) => string;
    empty: string;
    undrawn: (count: number) => string;
  };
}) {
  const [heights, setHeights] = useState<Record<string, number>>({});
  const onHeight = useCallback((id: string, height: number) => {
    if (height <= 0) return;
    setHeights((current) => current[id] === height ? current : { ...current, [id]: height });
  }, []);
  const layout = layoutGraph(graph, "all", heights);
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
              <path
                d="M 0 0 L 8 4 L 0 8 z"
                className="fill-muted-foreground/60"
              />
            </marker>
          </defs>

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
                  strokeDasharray={
                    laidOutEdge.edge.satisfied ? undefined : "5 4"
                  }
                  className={cn(
                    selected
                      ? "stroke-foreground"
                      : laidOutEdge.edge.satisfied
                        ? "stroke-muted-foreground"
                        : "stroke-muted-foreground/60",
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
            const state = labels.status(laidOut.node);
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
                className="group/graph-node cursor-pointer outline-none"
                onClick={() => selectNode(laidOut)}
                onKeyDown={(event) => {
                  if (event.key !== "Enter" && event.key !== " ") return;
                  event.preventDefault();
                  selectNode(laidOut);
                }}
              >
                <foreignObject
                  x={laidOut.x}
                  y={laidOut.y}
                  width={laidOut.width}
                  height={laidOut.height}
                  className="overflow-visible"
                >
                  <GraphCard node={laidOut.node} selected={selected} onHeight={onHeight} />
                </foreignObject>
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
