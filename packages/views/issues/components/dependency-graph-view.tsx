"use client";

import {
  Check,
  Maximize2,
  Move,
  Network,
  RotateCcw,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent,
  type ReactNode,
  type WheelEvent,
} from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { dependencyGraphKeys, dependencyGraphsOptions } from "@patchbay/core/dependency-graphs";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { useWSReconnect, useWSEvent } from "@patchbay/core/realtime";
import type {
  DependencyGraphEdge,
  DependencyGraphNode,
  DependencyGraphReadinessState,
  DependencyGraphResponse,
} from "@patchbay/core/types";
import { cn } from "@patchbay/ui/lib/utils";
import { Button } from "@patchbay/ui/components/ui/button";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { AppLink } from "../../navigation";
import { ActorAvatar } from "../../common/actor-avatar";
import { useT } from "../../i18n";
import { CustomStatusChip } from "./custom-status-chip";
import { StatusIcon } from "./status-icon";

const NODE_WIDTH = 236;
const NODE_HEIGHT = 126;
const COLUMN_GAP = 84;
const ROW_GAP = 28;
const GRAPH_GAP = 96;
const CANVAS_PADDING = 28;
const MIN_SCALE = 0.55;
const MAX_SCALE = 1.6;

type GraphFilter = "all" | "ready" | "running" | "blocked";
type SelectedNode = { planId: string; tempId: string };
type SelectedEdge = { planId: string; edgeId: string };
type Point = { x: number; y: number };
type PositionedNode = {
  graph: DependencyGraphResponse;
  node: DependencyGraphNode;
  x: number;
  y: number;
};

type ReadinessTranslationKey = "todo" | "ready" | "running" | "blocked" | "done" | "cancelled";

const READINESS_KEYS: Record<string, ReadinessTranslationKey> = {
  todo: "todo",
  ready: "ready",
  running: "running",
  blocked: "blocked",
  done: "done",
  cancelled: "cancelled",
};

const FILTER_KEYS: Record<GraphFilter, "all" | "ready" | "running" | "blocked"> = {
  all: "all",
  ready: "ready",
  running: "running",
  blocked: "blocked",
};

function nodeKey(planId: string, tempId: string): string {
  return `${planId}:${tempId}`;
}

function edgeKey(planId: string, edgeId: string): string {
  return `${planId}:${edgeId}`;
}

function clampScale(value: number): number {
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, value));
}

function nodeMatchesFilter(node: DependencyGraphNode, filter: GraphFilter): boolean {
  return filter === "all" || node.readiness.state === filter;
}

function stateLabel(
  t: ReturnType<typeof useT<"issues">>["t"],
  state: DependencyGraphReadinessState | string,
): string {
  // Keep an unknown server state visible as a safe, explicit fallback rather
  // than rendering an undefined locale value when the API grows first.
  return t(($) => $.graph.readiness_state[READINESS_KEYS[state] ?? "todo"]);
}

function formatAssignee(node: DependencyGraphNode): string | null {
  if (!node.assignee_type || !node.assignee_id) return null;
  return `${node.assignee_type}:${node.assignee_id.slice(0, 8)}`;
}

function layoutGraphs(graphs: DependencyGraphResponse[]): {
  nodes: PositionedNode[];
  nodePositions: Map<string, PositionedNode>;
  width: number;
  height: number;
} {
  const positioned: PositionedNode[] = [];
  let graphOffset = CANVAS_PADDING;
  let maxHeight = CANVAS_PADDING + NODE_HEIGHT;

  for (const graph of graphs) {
    const byWave = new Map<number, DependencyGraphNode[]>();
    for (const node of graph.nodes) {
      const wave = byWave.get(node.wave);
      if (wave) wave.push(node);
      else byWave.set(node.wave, [node]);
    }
    const waves = Array.from(byWave.keys()).sort((left, right) => left - right);
    const graphHeight = Math.max(
      NODE_HEIGHT,
      ...waves.map((wave) => (byWave.get(wave)?.length ?? 1) * (NODE_HEIGHT + ROW_GAP)),
    );
    for (const wave of waves) {
      const nodes = byWave.get(wave) ?? [];
      nodes.sort((left, right) => left.temp_id.localeCompare(right.temp_id));
      nodes.forEach((node, index) => {
        positioned.push({
          graph,
          node,
          x: graphOffset + wave * (NODE_WIDTH + COLUMN_GAP),
          y: CANVAS_PADDING + index * (NODE_HEIGHT + ROW_GAP),
        });
      });
    }
    const waveCount = Math.max(1, waves.length);
    graphOffset += waveCount * (NODE_WIDTH + COLUMN_GAP) + GRAPH_GAP;
    maxHeight = Math.max(maxHeight, CANVAS_PADDING + graphHeight);
  }

  const nodePositions = new Map<string, PositionedNode>();
  for (const item of positioned) {
    nodePositions.set(nodeKey(item.graph.plan.id, item.node.temp_id), item);
  }
  return {
    nodes: positioned,
    nodePositions,
    width: Math.max(720, graphOffset),
    height: Math.max(360, maxHeight),
  };
}

function relatedNodeKeys(
  graph: DependencyGraphResponse,
  selected: SelectedNode | null,
): { upstream: Set<string>; downstream: Set<string> } {
  const upstream = new Set<string>();
  const downstream = new Set<string>();
  if (!selected || selected.planId !== graph.plan.id) return { upstream, downstream };

  const reverse = new Map<string, string[]>();
  const forward = new Map<string, string[]>();
  for (const edge of graph.edges) {
    const previous = reverse.get(edge.to) ?? [];
    previous.push(edge.from);
    reverse.set(edge.to, previous);
    const next = forward.get(edge.from) ?? [];
    next.push(edge.to);
    forward.set(edge.from, next);
  }

  const visit = (start: string, adjacency: Map<string, string[]>, output: Set<string>) => {
    const pending = [...(adjacency.get(start) ?? [])];
    while (pending.length > 0) {
      const current = pending.shift();
      if (!current || !output.add(current)) continue;
      pending.push(...(adjacency.get(current) ?? []));
    }
  };
  visit(selected.tempId, reverse, upstream);
  visit(selected.tempId, forward, downstream);
  return { upstream, downstream };
}

export function DependencyGraphView({ projectId }: { projectId?: string }) {
  const { t } = useT("issues");
  const wsId = useWorkspaceId();
  const queryClient = useQueryClient();
  const paths = useWorkspacePaths();
  const query = useQuery({
    ...dependencyGraphsOptions(wsId, projectId),
    enabled: wsId.length > 0,
  });
  const [filter, setFilter] = useState<GraphFilter>("all");
  const [selectedNode, setSelectedNode] = useState<SelectedNode | null>(null);
  const [selectedEdge, setSelectedEdge] = useState<SelectedEdge | null>(null);
  const [scale, setScale] = useState(1);
  const [offset, setOffset] = useState<Point>({ x: 0, y: 0 });
  const [dragStart, setDragStart] = useState<Point | null>(null);
  const viewportRef = useRef<HTMLDivElement>(null);

  const invalidateGraphs = useCallback(() => {
    if (!wsId) return;
    queryClient.invalidateQueries({ queryKey: dependencyGraphKeys.all(wsId) });
  }, [queryClient, wsId]);
  useWSEvent("dependency_graph:updated", invalidateGraphs);
  useWSReconnect(invalidateGraphs);

  const graphs = query.data ?? [];
  const layout = useMemo(() => layoutGraphs(graphs), [graphs]);
  const selectedGraph = selectedNode
    ? graphs.find((graph) => graph.plan.id === selectedNode.planId)
    : undefined;
  const selectedNodeData = selectedGraph?.nodes.find(
    (node) => node.temp_id === selectedNode?.tempId,
  );
  const selectedEdgeData = selectedEdge
    ? graphs
        .find((graph) => graph.plan.id === selectedEdge.planId)
        ?.edges.find((edge) => edge.id === selectedEdge.edgeId)
    : undefined;
  const selectedEdgeGraph = selectedEdge
    ? graphs.find((graph) => graph.plan.id === selectedEdge.planId)
    : undefined;

  useEffect(() => {
    if (selectedNode && !selectedNodeData) setSelectedNode(null);
    if (selectedEdge && !selectedEdgeData) setSelectedEdge(null);
  }, [selectedEdge, selectedEdgeData, selectedNode, selectedNodeData]);

  const zoomBy = useCallback((delta: number) => {
    setScale((current) => clampScale(current + delta));
  }, []);

  const fitToView = useCallback(() => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    const availableWidth = Math.max(1, viewport.clientWidth - CANVAS_PADDING * 2);
    const availableHeight = Math.max(1, viewport.clientHeight - CANVAS_PADDING * 2);
    const nextScale = clampScale(
      Math.min(1, availableWidth / layout.width, availableHeight / layout.height),
    );
    setScale(nextScale);
    setOffset({ x: CANVAS_PADDING, y: CANVAS_PADDING });
  }, [layout.height, layout.width]);

  const resetViewport = useCallback(() => {
    setScale(1);
    setOffset({ x: 0, y: 0 });
  }, []);

  const handlePointerDown = useCallback((event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    const target = event.target as HTMLElement;
    if (target.closest("[data-graph-node], [data-graph-edge]")) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    setDragStart({ x: event.clientX - offset.x, y: event.clientY - offset.y });
  }, [offset.x, offset.y]);

  const handlePointerMove = useCallback((event: PointerEvent<HTMLDivElement>) => {
    if (!dragStart) return;
    setOffset({ x: event.clientX - dragStart.x, y: event.clientY - dragStart.y });
  }, [dragStart]);

  const handlePointerUp = useCallback((event: PointerEvent<HTMLDivElement>) => {
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    setDragStart(null);
  }, []);

  const handleWheel = useCallback((event: WheelEvent<HTMLDivElement>) => {
    event.preventDefault();
    zoomBy(event.deltaY < 0 ? 0.08 : -0.08);
  }, [zoomBy]);

  const nodeRelation = useMemo(() => {
    const upstream = new Set<string>();
    const downstream = new Set<string>();
    for (const graph of graphs) {
      const related = relatedNodeKeys(graph, selectedNode);
      for (const key of related.upstream) upstream.add(nodeKey(graph.plan.id, key));
      for (const key of related.downstream) downstream.add(nodeKey(graph.plan.id, key));
    }
    return { upstream, downstream };
  }, [graphs, selectedNode]);

  const visibleNodeKeys = useMemo(
    () =>
      new Set(
        layout.nodes
          .filter((item) => nodeMatchesFilter(item.node, filter))
          .map((item) => nodeKey(item.graph.plan.id, item.node.temp_id)),
      ),
    [filter, layout.nodes],
  );

  const totals = useMemo(
    () =>
      graphs.reduce(
        (summary, graph) => ({
          total: summary.total + graph.readiness.total,
          ready: summary.ready + graph.readiness.ready,
          running: summary.running + graph.readiness.running,
          blocked: summary.blocked + graph.readiness.blocked,
        }),
        { total: 0, ready: 0, running: 0, blocked: 0 },
      ),
    [graphs],
  );
  const attentionReason = graphs.find((graph) => graph.plan.attention_required)?.plan.attention_reason;

  if (query.isLoading) {
    return (
      <div className="flex flex-1 items-center justify-center p-8 text-caption text-muted-foreground">
        {t(($) => $.graph.loading)}
      </div>
    );
  }

  if (query.isError) {
    return (
      <div className="flex flex-1 flex-col items-center justify-center gap-3 p-8 text-muted-foreground" role="alert">
        <Network className="size-10 text-faint-foreground" />
        <p className="text-body">{t(($) => $.graph.load_failed)}</p>
        <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
          {t(($) => $.graph.retry)}
        </Button>
      </div>
    );
  }

  if (graphs.length === 0) {
    return (
      <div className="flex flex-1 flex-col items-center justify-center gap-3 p-8 text-muted-foreground">
        <Network className="size-10 text-faint-foreground" />
        <p className="text-body">{t(($) => $.graph.empty_title)}</p>
        <p className="max-w-md text-center text-caption">{t(($) => $.graph.empty_hint)}</p>
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3 p-3 md:p-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-2 text-caption text-muted-foreground">
          <Network className="size-4" />
          <span>{t(($) => $.graph.active_plans, { count: graphs.length })}</span>
          <span aria-hidden>·</span>
          <span>{t(($) => $.graph.task_summary, totals)}</span>
        </div>
        <div className="flex flex-wrap items-center gap-1" role="toolbar" aria-label={t(($) => $.graph.toolbar)}>
          {(Object.keys(FILTER_KEYS) as GraphFilter[]).map((value) => (
            <Button
              key={value}
              variant={filter === value ? "secondary" : "ghost"}
              size="sm"
              className="h-7 px-2 text-caption"
              aria-pressed={filter === value}
              onClick={() => setFilter(value)}
            >
              {t(($) => $.graph.filter[FILTER_KEYS[value]])}
            </Button>
          ))}
          <span className="mx-1 h-4 w-px bg-border" aria-hidden />
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={t(($) => $.graph.zoom_out)}
            onClick={() => zoomBy(-0.1)}
          >
            <ZoomOut className="size-3.5" />
          </Button>
          <span className="w-11 text-center text-micro tabular-nums text-muted-foreground">
            {Math.round(scale * 100)}%
          </span>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={t(($) => $.graph.zoom_in)}
            onClick={() => zoomBy(0.1)}
          >
            <ZoomIn className="size-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={t(($) => $.graph.fit)}
            onClick={fitToView}
          >
            <Maximize2 className="size-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={t(($) => $.graph.reset)}
            onClick={resetViewport}
          >
            <RotateCcw className="size-3.5" />
          </Button>
        </div>
      </div>

      {attentionReason && (
        <div
          role="alert"
          className="rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-caption text-amber-800 dark:text-amber-200"
        >
          {t(($) => $.graph.attention, { reason: attentionReason })}
        </div>
      )}

      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-hidden lg:flex-row">
        <div
          ref={viewportRef}
          className={cn(
            "relative min-h-[360px] min-w-0 flex-1 overflow-hidden rounded-lg border border-surface-border bg-surface-subtle",
            dragStart ? "cursor-grabbing" : "cursor-grab",
          )}
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={handlePointerUp}
          onPointerCancel={handlePointerUp}
          onWheel={handleWheel}
          data-graph-viewport
        >
          <div className="pointer-events-none absolute left-3 top-3 z-20 flex items-center gap-1.5 rounded-md bg-surface/90 px-2 py-1 text-micro text-muted-foreground shadow-sm">
            <Move className="size-3" />
            {t(($) => $.graph.pan_hint)}
          </div>
          <div
            className="absolute left-0 top-0 origin-top-left transition-transform duration-100"
            style={{
              width: layout.width,
              height: layout.height,
              transform: `translate(${offset.x}px, ${offset.y}px) scale(${scale})`,
            }}
            data-graph-canvas
          >
            <svg
              className="pointer-events-none absolute inset-0 overflow-visible"
              width={layout.width}
              height={layout.height}
              role="group"
              aria-label={t(($) => $.graph.canvas_label)}
            >
              {graphs.flatMap((graph) =>
                graph.edges.map((edge) => {
                  const sourceKey = nodeKey(graph.plan.id, edge.from);
                  const targetKey = nodeKey(graph.plan.id, edge.to);
                  if (!visibleNodeKeys.has(sourceKey) || !visibleNodeKeys.has(targetKey)) return null;
                  const source = layout.nodePositions.get(nodeKey(graph.plan.id, edge.from));
                  const target = layout.nodePositions.get(nodeKey(graph.plan.id, edge.to));
                  if (!source || !target) return null;
                  const isSelected = selectedEdge?.planId === graph.plan.id && selectedEdge.edgeId === edge.id;
                  const isRelated = selectedNode
                    ? selectedNode.planId === graph.plan.id &&
                      (selectedNode.tempId === edge.from || selectedNode.tempId === edge.to ||
                        nodeRelation.upstream.has(sourceKey) || nodeRelation.downstream.has(targetKey))
                    : true;
                  const x1 = source.x + NODE_WIDTH;
                  const y1 = source.y + NODE_HEIGHT / 2;
                  const x2 = target.x;
                  const y2 = target.y + NODE_HEIGHT / 2;
                  const midX = x1 + (x2 - x1) / 2;
                  return (
                    <g
                      key={edge.id}
                      data-graph-edge
                      role="button"
                      tabIndex={0}
                      aria-label={t(($) => $.graph.edge_label, {
                        from: source.node.issue.identifier,
                        to: target.node.issue.identifier,
                      })}
                      className={cn(
                        "pointer-events-auto cursor-pointer outline-none",
                        !isRelated && "opacity-20",
                      )}
                      onClick={() => {
                        setSelectedEdge({ planId: graph.plan.id, edgeId: edge.id });
                        setSelectedNode(null);
                      }}
                      onKeyDown={(event: KeyboardEvent<SVGGElement>) => {
                        if (event.key === "Enter" || event.key === " ") {
                          event.preventDefault();
                          setSelectedEdge({ planId: graph.plan.id, edgeId: edge.id });
                          setSelectedNode(null);
                        }
                      }}
                    >
                      <path
                        d={`M ${x1} ${y1} C ${midX} ${y1}, ${midX} ${y2}, ${x2} ${y2}`}
                        fill="none"
                        stroke="transparent"
                        strokeWidth={18}
                      />
                      <path
                        d={`M ${x1} ${y1} C ${midX} ${y1}, ${midX} ${y2}, ${x2} ${y2}`}
                        fill="none"
                        stroke="currentColor"
                        strokeWidth={isSelected ? 2.5 : 1.5}
                        strokeDasharray={edge.satisfied ? "4 4" : undefined}
                        className={cn(
                          edge.satisfied ? "text-emerald-500/70" : "text-muted-foreground/70",
                          isSelected && "text-brand",
                        )}
                      />
                      <path
                        d={`M ${x2 - 7} ${y2 - 4} L ${x2} ${y2} L ${x2 - 7} ${y2 + 4}`}
                        fill="none"
                        stroke="currentColor"
                        strokeWidth={isSelected ? 2.5 : 1.5}
                        className={cn(
                          edge.satisfied ? "text-emerald-500/70" : "text-muted-foreground/70",
                          isSelected && "text-brand",
                        )}
                      />
                    </g>
                  );
                }),
              )}
            </svg>

            {layout.nodes.filter((item) => visibleNodeKeys.has(nodeKey(item.graph.plan.id, item.node.temp_id))).map((item) => {
              const key = nodeKey(item.graph.plan.id, item.node.temp_id);
              const isSelected = selectedNode?.planId === item.graph.plan.id && selectedNode.tempId === item.node.temp_id;
              const isRelated = !selectedNode || isSelected || nodeRelation.upstream.has(key) || nodeRelation.downstream.has(key);
              const readinessLabel = stateLabel(t, item.node.readiness.state);
              const assignee = formatAssignee(item.node);
              return (
                <AppLink
                  key={key}
                  href={paths.issueDetail(item.node.issue.identifier || item.node.issue.id)}
                  newTabTitle={item.node.issue.identifier}
                  data-graph-node
                  className={cn(
                    "absolute flex flex-col rounded-lg border bg-surface px-3 py-2.5 text-left shadow-sm outline-none transition-[opacity,box-shadow,border-color] focus-visible:ring-2 focus-visible:ring-brand/60",
                    item.node.readiness.state === "blocked"
                      ? "border-amber-500/40"
                      : item.node.readiness.state === "done"
                        ? "border-emerald-500/40"
                        : "border-surface-border",
                    isSelected && "border-brand ring-2 ring-brand/30",
                    isRelated ? "opacity-100" : "opacity-25",
                  )}
                  style={{ left: item.x, top: item.y, width: NODE_WIDTH, height: NODE_HEIGHT }}
                  onClick={() => {
                    setSelectedNode({ planId: item.graph.plan.id, tempId: item.node.temp_id });
                    setSelectedEdge(null);
                  }}
                  aria-label={t(($) => $.graph.node_label, {
                    identifier: item.node.issue.identifier,
                    title: item.node.title,
                    status: item.node.issue.status,
                    readiness: readinessLabel,
                    assignee: assignee ?? t(($) => $.graph.unassigned),
                  })}
                >
                  <span className="flex min-w-0 items-center justify-between gap-2">
                    <span className="flex min-w-0 items-center gap-1.5">
                      <StatusIcon status={item.node.issue.status} className="size-3.5 shrink-0" />
                      <span className="truncate text-micro text-muted-foreground">
                        {item.node.issue.identifier}
                      </span>
                    </span>
                    {item.node.readiness.state === "done" && (
                      <Check className="size-3.5 shrink-0 text-emerald-500" aria-label={t(($) => $.graph.satisfied)} />
                    )}
                  </span>
                  <span className="mt-1 line-clamp-2 text-caption font-medium leading-snug">
                    {item.node.title}
                  </span>
                  <span className="mt-auto flex min-w-0 items-center justify-between gap-2">
                    <span className="flex min-w-0 items-center gap-1 text-micro text-muted-foreground">
                      <span className="truncate">{readinessLabel}</span>
                      <span aria-hidden>·</span>
                      <span className="tabular-nums">
                        {item.node.readiness.satisfied_prerequisites}/{item.node.readiness.total_prerequisites}
                      </span>
                    </span>
                    {assignee && item.node.assignee_type && item.node.assignee_id ? (
                      <ActorAvatar
                        actorType={item.node.assignee_type}
                        actorId={item.node.assignee_id}
                        size="xs"
                        profileLink={false}
                        showStatusDot={item.node.assignee_type === "agent"}
                        className="shrink-0"
                      />
                    ) : null}
                  </span>
                </AppLink>
              );
            })}
          </div>
        </div>

        <aside className="w-full shrink-0 rounded-lg border border-surface-border bg-surface p-4 lg:w-80" aria-live="polite">
          {selectedEdgeData && selectedEdgeGraph ? (
            <DependencyGateInspector
              edge={selectedEdgeData}
              graph={selectedEdgeGraph}
              t={t}
              paths={paths}
            />
          ) : selectedNodeData && selectedGraph ? (
            <NodeInspector node={selectedNodeData} t={t} paths={paths} />
          ) : (
            <div className="flex h-full min-h-36 flex-col items-center justify-center gap-2 text-center text-muted-foreground">
              <Network className="size-7 text-faint-foreground" />
              <p className="text-caption">{t(($) => $.graph.select_hint)}</p>
            </div>
          )}
        </aside>
      </div>
    </div>
  );
}

function NodeInspector({
  node,
  t,
  paths,
}: {
  node: DependencyGraphNode;
  t: ReturnType<typeof useT<"issues">>["t"];
  paths: ReturnType<typeof useWorkspacePaths>;
}) {
  const assignee = formatAssignee(node);
  return (
    <div className="space-y-3">
      <div>
        <p className="text-micro uppercase tracking-wide text-muted-foreground">{t(($) => $.graph.task)}</p>
        <AppLink href={paths.issueDetail(node.issue.identifier || node.issue.id)} className="mt-1 block text-body font-medium hover:underline">
          {node.issue.identifier} · {node.title}
        </AppLink>
      </div>
      <div className="flex items-center gap-2">
        <StatusIcon status={node.issue.status} className="size-4" />
        <span className="text-caption">{stateLabel(t, node.readiness.state)}</span>
        <CustomStatusChip status={node.issue.status} />
      </div>
      <InspectorRow label={t(($) => $.graph.assignee)}>
        {assignee && node.assignee_type && node.assignee_id ? (
          <span className="flex items-center gap-1.5">
            <ActorAvatar actorType={node.assignee_type} actorId={node.assignee_id} size="xs" profileLink={false} />
            <span className="truncate">{assignee}</span>
          </span>
        ) : (
          t(($) => $.graph.unassigned)
        )}
      </InspectorRow>
      <InspectorRow label={t(($) => $.graph.readiness)}>
        <span className="tabular-nums">
          {node.readiness.satisfied_prerequisites}/{node.readiness.total_prerequisites}
        </span>{" "}
        {t(($) => $.graph.prerequisites)}
      </InspectorRow>
      <p className="rounded-md bg-muted/50 p-2 text-micro text-muted-foreground">
        {node.readiness.unlock_condition}
      </p>
    </div>
  );
}

function DependencyGateInspector({
  edge,
  graph,
  t,
  paths,
}: {
  edge: DependencyGraphEdge;
  graph: DependencyGraphResponse;
  t: ReturnType<typeof useT<"issues">>["t"];
  paths: ReturnType<typeof useWorkspacePaths>;
}) {
  const source = graph.nodes.find((node) => node.temp_id === edge.from);
  const target = graph.nodes.find((node) => node.temp_id === edge.to);
  return (
    <div className="space-y-3">
      <div>
        <p className="text-micro uppercase tracking-wide text-muted-foreground">{t(($) => $.graph.edge_inspector)}</p>
        <p className="mt-1 text-caption font-medium">
          {source && target ? (
            <>
              <AppLink href={paths.issueDetail(source.issue.identifier || source.issue.id)} className="hover:underline">
                {source.issue.identifier}
              </AppLink>{" "}
              →{" "}
              <AppLink href={paths.issueDetail(target.issue.identifier || target.issue.id)} className="hover:underline">
                {target.issue.identifier}
              </AppLink>
            </>
          ) : (
            `${edge.from} → ${edge.to}`
          )}
        </p>
      </div>
      <InspectorRow label={t(($) => $.graph.reason)}>{edge.reason}</InspectorRow>
      <InspectorRow label={t(($) => $.graph.consumed_output)}>{edge.consumed_output}</InspectorRow>
      <InspectorRow label={t(($) => $.graph.prerequisite_status)}>
        <span className={cn(edge.satisfied ? "text-emerald-600 dark:text-emerald-400" : "text-amber-600 dark:text-amber-400")}>
          {edge.prerequisite_status}
        </span>
      </InspectorRow>
      <InspectorRow label={t(($) => $.graph.satisfied_count)}>
        <span className="tabular-nums">
          {edge.satisfied_prerequisites}/{edge.total_prerequisites}
        </span>
      </InspectorRow>
      <div className={cn("rounded-md p-2 text-micro", edge.satisfied ? "bg-emerald-500/10 text-emerald-700 dark:text-emerald-300" : "bg-amber-500/10 text-amber-700 dark:text-amber-300")}>
        {edge.satisfied ? t(($) => $.graph.satisfied) : edge.unlock_condition}
      </div>
    </div>
  );
}

function InspectorRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="space-y-1">
      <p className="text-micro text-muted-foreground">{label}</p>
      <div className="break-words text-caption">{children}</div>
    </div>
  );
}
