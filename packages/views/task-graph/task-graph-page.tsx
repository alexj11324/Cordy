"use client";

import { useCallback, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  CheckCircle2,
  CircleDashed,
  Network,
  RefreshCw,
} from "lucide-react";
import {
  dependencyGraphKeys,
  dependencyGraphsOptions,
} from "@patchbay/core/dependency-graphs";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { projectListOptions } from "@patchbay/core/projects";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { useWSReconnect, useWSEvent } from "@patchbay/core/realtime";
import type {
  DependencyGraphNode,
  DependencyGraphReadinessState,
  DependencyGraphResponse,
} from "@patchbay/core/types";
import { Button } from "@patchbay/ui/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@patchbay/ui/components/ui/select";
import { cn } from "@patchbay/ui/lib/utils";
import {
  CollectionPageHeader,
  CollectionPageHeaderAction,
  CollectionPageState,
} from "../layout";
import { AppLink } from "../navigation";
import { useT } from "../i18n";
import {
  edgeEndpoint,
  nodeMatchesFilter,
  summarizeGraphs,
  type GraphFilter,
} from "./graph-utils";
import { GraphCanvas } from "./graph-canvas";

const EMPTY_GRAPHS: DependencyGraphResponse[] = [];

/** Sentinel for "no project filter"; an empty Select value is not selectable. */
const ALL_PROJECTS = "__all__";

type ViewMode = "graph" | "list";

type Selection =
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

function nodeReadiness(node: DependencyGraphNode) {
  return node.readiness ?? {
    state: "todo",
    gate_open: false,
    satisfied_prerequisites: 0,
    total_prerequisites: 0,
    unlock_condition: "",
  };
}

function nodeActor(node: DependencyGraphNode): string | null {
  const type = node.executor_type;
  const id = node.executor_id;
  if (!type || !id) return null;
  return `${type}:${id.slice(0, 8)}`;
}

function edgeLabel(graph: DependencyGraphResponse, endpoint: string): string {
  const node = graph.nodes.find(
    (candidate) =>
      candidate.temp_id === endpoint ||
      candidate.issue_id === endpoint ||
      candidate.issue?.id === endpoint ||
      candidate.issue?.identifier === endpoint,
  );
  return node ? nodeIdentifier(node) : endpoint || "?";
}

function graphNodeId(node: DependencyGraphNode): string {
  return node.id || node.temp_id || node.issue_id;
}

function stateClass(state: string): string {
  switch (state) {
    case "ready":
      return "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300";
    case "running":
      return "border-blue-500/30 bg-blue-500/10 text-blue-700 dark:text-blue-300";
    case "blocked":
      return "border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300";
    case "done":
      return "border-muted bg-muted text-muted-foreground";
    default:
      return "border-border bg-background text-muted-foreground";
  }
}

function statusLabel(
  state: string,
  t: ReturnType<typeof useT<"task-graph">>["t"],
): string {
  const labels: Record<DependencyGraphReadinessState, () => string> = {
    todo: () => t(($) => $.todo),
    ready: () => t(($) => $.ready),
    running: () => t(($) => $.running),
    blocked: () => t(($) => $.blocked),
    done: () => t(($) => $.done),
    cancelled: () => t(($) => $.cancelled),
  };
  return labels[state as DependencyGraphReadinessState]?.() ?? state;
}

function PlanSection({
  graph,
  filter,
  view,
  selection,
  onSelect,
  t,
  paths,
}: {
  graph: DependencyGraphResponse;
  filter: GraphFilter;
  view: ViewMode;
  selection: Selection;
  onSelect: (selection: Selection) => void;
  t: ReturnType<typeof useT<"task-graph">>["t"];
  paths: ReturnType<typeof useWorkspacePaths>;
}) {
  const nodes = graph.nodes.filter((node) => nodeMatchesFilter(node, filter));
  const waves = Array.from(new Set(nodes.map((node) => node.wave))).sort(
    (left, right) => left - right,
  );

  return (
    <section
      className="rounded-xl border border-border/70 bg-card/40 p-4"
      aria-labelledby={`dependency-plan-${graph.plan.id}`}
    >
      <div className="flex flex-wrap items-start justify-between gap-3 border-b border-border/60 pb-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <Network aria-hidden="true" className="size-4 text-muted-foreground" />
            <h2
              id={`dependency-plan-${graph.plan.id}`}
              className="text-body font-medium"
            >
              {t(($) => $.plan)} · {graph.plan.id.slice(0, 8)}
            </h2>
            <span
              className={cn(
                "rounded-full border px-2 py-0.5 text-caption",
                stateClass(graph.plan.status),
              )}
            >
              {statusLabel(graph.plan.status, t)}
            </span>
          </div>
          <p className="mt-1 text-caption text-muted-foreground">
            {graph.plan.goal || t(($) => $.goal)}
          </p>
        </div>
        <span className="text-caption text-muted-foreground">
          {t(($) => $.task_summary, {
            total: graph.readiness.total || graph.nodes.length,
            ready: graph.readiness.ready,
            running: graph.readiness.running,
            blocked: graph.readiness.blocked,
          })}
        </span>
      </div>

      {graph.plan.attention_required ? (
        <div className="mt-3 flex items-start gap-2 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-caption text-amber-800 dark:text-amber-200">
          <AlertTriangle aria-hidden="true" className="mt-0.5 size-3.5 shrink-0" />
          <span>
            {t(($) => $.attention, {
              reason: graph.plan.attention_reason || t(($) => $.status),
            })}
          </span>
        </div>
      ) : null}

      {view === "graph" ? (
        <GraphCanvas
          graph={graph}
          filter={filter}
          selection={selection}
          onSelect={onSelect}
          labels={{
            canvas: t(($) => $.canvas, { plan: graph.plan.id.slice(0, 8) }),
            nodeHint: (args) => t(($) => $.canvas_node, args),
            edgeHint: ({ from, to, satisfied }) =>
              t(($) => $.canvas_edge, {
                from,
                to,
                status: satisfied
                  ? t(($) => $.satisfied)
                  : t(($) => $.blocked),
              }),
            waveColumn: (wave) => t(($) => $.wave, { count: wave }),
            empty: t(($) => $.no_matching_tasks),
            undrawn: (count) => t(($) => $.canvas_undrawn, { count }),
          }}
        />
      ) : waves.length > 0 ? (
        <div className="mt-4 grid gap-3 md:grid-cols-2 xl:grid-cols-3">
          {waves.map((wave) => {
            const waveNodes = nodes
              .filter((node) => node.wave === wave)
              .sort((left, right) => nodeTitle(left).localeCompare(nodeTitle(right)));
            return (
              <div key={wave} className="min-w-0 rounded-lg bg-muted/25 p-3">
                <h3 className="mb-2 text-label font-medium text-muted-foreground">
                  {t(($) => $.wave, { count: wave })}
                </h3>
                <div className="space-y-2">
                  {waveNodes.map((node) => {
                    const readiness = nodeReadiness(node);
                    const selected =
                      selection?.kind === "node" &&
                      selection.planId === graph.plan.id &&
                      selection.nodeId === graphNodeId(node);
                    const identifier = nodeIdentifier(node);
                    return (
                      <article
                        key={graphNodeId(node)}
                        data-testid="dependency-graph-node"
                        className={cn(
                          "rounded-lg border border-border bg-background p-3 shadow-xs",
                          selected && "border-brand ring-1 ring-brand/30",
                        )}
                      >
                        <div className="flex items-start justify-between gap-2">
                          <AppLink
                            href={paths.issueDetail(identifier)}
                            newTabTitle={identifier}
                            className="min-w-0 truncate text-label font-medium text-primary hover:underline"
                          >
                            {identifier}
                          </AppLink>
                          <span
                            className={cn(
                              "shrink-0 rounded-full border px-1.5 py-0.5 text-micro",
                              stateClass(nodeState(node)),
                            )}
                          >
                            {statusLabel(nodeState(node), t)}
                          </span>
                        </div>
                        <p className="mt-1 line-clamp-2 text-label text-foreground">
                          {nodeTitle(node)}
                        </p>
                        <div className="mt-2 flex flex-wrap items-center gap-x-2 gap-y-1 text-caption text-muted-foreground">
                          <span>
                            {readiness.gate_open
                              ? t(($) => $.gate_open)
                              : t(($) => $.gate_blocked)}
                          </span>
                          <span aria-hidden="true">·</span>
                          <span>
                            {t(($) => $.prerequisites, {
                              satisfied: readiness.satisfied_prerequisites,
                              total: readiness.total_prerequisites,
                            })}
                          </span>
                        </div>
                        <div className="mt-3 flex items-center justify-between gap-2">
                          <span className="truncate text-caption text-muted-foreground">
                            {nodeActor(node) ?? t(($) => $.unassigned)}
                          </span>
                          <Button
                            type="button"
                            size="xs"
                            variant={selected ? "brandSubtle" : "ghost"}
                            aria-pressed={selected}
                            onClick={() =>
                              onSelect({
                                kind: "node",
                                planId: graph.plan.id,
                                nodeId: graphNodeId(node),
                              })
                            }
                          >
                            {t(($) => $.inspector)}
                          </Button>
                        </div>
                      </article>
                    );
                  })}
                </div>
              </div>
            );
          })}
        </div>
      ) : (
        <p className="py-8 text-center text-caption text-muted-foreground">
          {t(($) => $.no_matching_tasks)}
        </p>
      )}

      {/* The canvas already draws every edge it can, so the textual list is
          the list view's job. It stays the readable fallback for edges the
          canvas leaves undrawn. */}
      <div className={cn("mt-4 border-t border-border/60 pt-3", view === "graph" && "hidden")}>
        <div className="mb-2 flex items-center gap-2 text-label font-medium">
          <Network aria-hidden="true" className="size-3.5 text-muted-foreground" />
          {t(($) => $.dependency)}
        </div>
        {graph.edges.length > 0 ? (
          <ul className="space-y-1.5">
            {graph.edges.map((edge) => {
              const selected =
                selection?.kind === "edge" &&
                selection.planId === graph.plan.id &&
                selection.edgeId === edge.id;
              return (
                <li key={edge.id}>
                  <Button
                    type="button"
                    variant={selected ? "brandSubtle" : "ghost"}
                    className="h-auto w-full justify-start px-2 py-1.5 text-left"
                    aria-pressed={selected}
                    onClick={() =>
                      onSelect({
                        kind: "edge",
                        planId: graph.plan.id,
                        edgeId: edge.id,
                      })
                    }
                  >
                    <span className="truncate">
                      {t(($) => $.dependency_from_to, {
                        from: edgeLabel(graph, edgeEndpoint(edge, "from")),
                        to: edgeLabel(graph, edgeEndpoint(edge, "to")),
                      })}
                    </span>
                    <span
                      className={cn(
                        "ml-auto shrink-0 text-caption",
                        edge.satisfied ? "text-emerald-600" : "text-amber-600",
                      )}
                    >
                      {edge.satisfied
                        ? t(($) => $.satisfied)
                        : t(($) => $.blocked)}
                    </span>
                  </Button>
                </li>
              );
            })}
          </ul>
        ) : (
          <p className="text-caption text-muted-foreground">
            {t(($) => $.no_dependencies)}
          </p>
        )}
      </div>
    </section>
  );
}

export function TaskGraphPage() {
  const { t } = useT("task-graph");
  const wsId = useWorkspaceId();
  const paths = useWorkspacePaths();
  const queryClient = useQueryClient();
  const [projectId, setProjectId] = useState<string>(ALL_PROJECTS);
  const [view, setView] = useState<ViewMode>("graph");
  const query = useQuery({
    ...dependencyGraphsOptions(
      wsId,
      projectId === ALL_PROJECTS ? undefined : projectId,
    ),
    enabled: wsId.length > 0,
  });
  const projects = useQuery({
    ...projectListOptions(wsId),
    enabled: wsId.length > 0,
  });
  const [filter, setFilter] = useState<GraphFilter>("all");
  const [selection, setSelection] = useState<Selection>(null);

  const invalidateGraphs = useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: dependencyGraphKeys.all(wsId) });
  }, [queryClient, wsId]);
  useWSEvent("dependency_graph:updated", invalidateGraphs);
  useWSReconnect(invalidateGraphs);

  // Base UI's Select needs the option list up front for typeahead and
  // keyboard navigation, separately from the rendered items.
  const projectSelectItems = useMemo(
    () => [
      { value: ALL_PROJECTS, label: t(($) => $.project.all) },
      ...(projects.data ?? []).map((project) => ({
        value: project.id,
        label: project.title,
      })),
    ],
    [projects.data, t],
  );

  const graphs = query.data ?? EMPTY_GRAPHS;
  const summary = useMemo(() => summarizeGraphs(graphs), [graphs]);
  const visibleGraphs = useMemo(
    () =>
      graphs.filter(
        (graph) =>
          filter === "all" || graph.nodes.some((node) => nodeMatchesFilter(node, filter)),
      ),
    [filter, graphs],
  );

  const selectedGraph = selection
    ? graphs.find((graph) => graph.plan.id === selection.planId)
    : undefined;
  const selectedNode =
    selection?.kind === "node"
      ? selectedGraph?.nodes.find((node) => graphNodeId(node) === selection.nodeId)
      : undefined;
  const selectedEdge =
    selection?.kind === "edge"
      ? selectedGraph?.edges.find((edge) => edge.id === selection.edgeId)
      : undefined;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <CollectionPageHeader
        icon={Network}
        title={t(($) => $.title)}
        count={graphs.length}
        description={t(($) => $.description)}
        actions={
          <CollectionPageHeaderAction
            icon={RefreshCw}
            label={t(($) => $.refresh)}
            aria-label={t(($) => $.refresh)}
            onClick={() => void query.refetch()}
            disabled={query.isFetching}
          />
        }
      />

      <div className="min-h-0 flex-1 overflow-auto">
        {query.isPending ? (
          <CollectionPageState
            icon={CircleDashed}
            title={t(($) => $.loading)}
            role="status"
          />
        ) : query.isError ? (
          <CollectionPageState
            icon={AlertTriangle}
            title={t(($) => $.load_failed)}
            tone="destructive"
            role="alert"
            actions={
              <Button type="button" variant="outline" onClick={() => void query.refetch()}>
                {t(($) => $.retry)}
              </Button>
            }
          />
        ) : graphs.length === 0 ? (
          <CollectionPageState
            icon={Network}
            title={t(($) => $.empty_title)}
            description={t(($) => $.empty_hint)}
          />
        ) : (
          <div className="mx-auto flex w-full max-w-[1600px] flex-col gap-4 p-4 lg:p-6">
            <div className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-border/70 bg-card/40 px-4 py-3">
              <div className="flex items-center gap-2 text-caption text-muted-foreground">
                <CheckCircle2 aria-hidden="true" className="size-4" />
                {t(($) => $.active_plans, { count: graphs.length })}
                <span aria-hidden="true">·</span>
                {t(($) => $.task_summary, summary)}
              </div>
              <div
                className="flex flex-wrap items-center gap-2"
                aria-label={t(($) => $.toolbar)}
              >
                <Select
                  items={projectSelectItems}
                  value={projectId}
                  onValueChange={(value) =>
                    setProjectId(value ?? ALL_PROJECTS)
                  }
                >
                  <SelectTrigger
                    size="sm"
                    className="w-[180px]"
                    aria-label={t(($) => $.project.label)}
                  >
                    <SelectValue placeholder={t(($) => $.project.all)} />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value={ALL_PROJECTS}>
                      {t(($) => $.project.all)}
                    </SelectItem>
                    {(projects.data ?? []).map((project) => (
                      <SelectItem key={project.id} value={project.id}>
                        {project.title}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>

                <div className="flex items-center gap-1">
                  {(["graph", "list"] as const).map((value) => (
                    <Button
                      key={value}
                      type="button"
                      size="xs"
                      variant={view === value ? "brandSubtle" : "ghost"}
                      aria-pressed={view === value}
                      onClick={() => setView(value)}
                    >
                      {t(($) => $.view[value])}
                    </Button>
                  ))}
                </div>

                <div className="flex flex-wrap items-center gap-1">
                  {(["all", "ready", "running", "blocked"] as const).map((value) => (
                    <Button
                      key={value}
                      type="button"
                      size="xs"
                      variant={filter === value ? "brandSubtle" : "ghost"}
                      aria-pressed={filter === value}
                      onClick={() => setFilter(value)}
                    >
                      {t(($) => $.filter[value])}
                    </Button>
                  ))}
                </div>
              </div>
            </div>

            {view === "graph" ? (
              <p className="text-caption text-muted-foreground">
                {t(($) => $.list_fallback_hint)}
              </p>
            ) : null}

            <div className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_280px]">
              <div className="space-y-4">
                {visibleGraphs.map((graph) => (
                  <PlanSection
                    key={graph.plan.id}
                    graph={graph}
                    filter={filter}
                    view={view}
                    selection={selection}
                    onSelect={setSelection}
                    t={t}
                    paths={paths}
                  />
                ))}
                {visibleGraphs.length === 0 ? (
                  <CollectionPageState
                    icon={Network}
                    title={t(($) => $.no_matching_tasks)}
                  />
                ) : null}
              </div>

              <aside className="h-fit rounded-xl border border-border/70 bg-card/40 p-4 xl:sticky xl:top-4">
                <h2 className="text-body font-medium">{t(($) => $.inspector)}</h2>
                {!selectedNode && !selectedEdge ? (
                  <p className="mt-2 text-caption text-muted-foreground">
                    {t(($) => $.select_hint)}
                  </p>
                ) : selectedNode ? (
                  <div className="mt-3 space-y-3 text-caption">
                    <div>
                      <AppLink
                        href={paths.issueDetail(nodeIdentifier(selectedNode))}
                        className="text-primary hover:underline"
                      >
                        {nodeIdentifier(selectedNode)}
                      </AppLink>
                      <p className="mt-1 text-body font-medium">{nodeTitle(selectedNode)}</p>
                    </div>
                    <dl className="space-y-2">
                      <div className="flex justify-between gap-3">
                        <dt className="text-muted-foreground">{t(($) => $.readiness)}</dt>
                        <dd className="font-medium">{statusLabel(nodeState(selectedNode), t)}</dd>
                      </div>
                      <div className="flex justify-between gap-3">
                        <dt className="text-muted-foreground">{t(($) => $.executor)}</dt>
                        <dd className="truncate">{nodeActor(selectedNode) ?? t(($) => $.unassigned)}</dd>
                      </div>
                      <div className="flex justify-between gap-3">
                        <dt className="text-muted-foreground">{t(($) => $.acceptance_count, { count: selectedNode.acceptance_criteria.length })}</dt>
                        <dd>{selectedNode.acceptance_criteria.length}</dd>
                      </div>
                    </dl>
                    {selectedNode.acceptance_criteria.length > 0 ? (
                      <ul className="list-disc space-y-1 pl-4 text-muted-foreground">
                        {selectedNode.acceptance_criteria.map((criterion, index) => (
                          <li key={`${index}-${criterion}`}>{criterion}</li>
                        ))}
                      </ul>
                    ) : (
                      <p className="text-muted-foreground">
                        {t(($) => $.no_acceptance_criteria)}
                      </p>
                    )}
                  </div>
                ) : selectedEdge && selectedGraph ? (
                  <dl className="mt-3 space-y-3 text-caption">
                    <div>
                      <dt className="text-muted-foreground">{t(($) => $.dependency)}</dt>
                      <dd className="mt-1 font-medium">
                        {t(($) => $.dependency_from_to, {
                          from: edgeLabel(selectedGraph, edgeEndpoint(selectedEdge, "from")),
                          to: edgeLabel(selectedGraph, edgeEndpoint(selectedEdge, "to")),
                        })}
                      </dd>
                    </div>
                    <div>
                      <dt className="text-muted-foreground">{t(($) => $.dependency_reason)}</dt>
                      <dd className="mt-1">{selectedEdge.reason || t(($) => $.status)}</dd>
                    </div>
                    {selectedEdge.consumed_output ? (
                      <div>
                        <dt className="text-muted-foreground">{t(($) => $.consumed_output)}</dt>
                        <dd className="mt-1">{selectedEdge.consumed_output}</dd>
                      </div>
                    ) : null}
                  </dl>
                ) : null}
              </aside>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
