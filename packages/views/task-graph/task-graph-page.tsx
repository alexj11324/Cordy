"use client";

import { useCallback, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowLeft,
  AlertTriangle,
  CheckCircle2,
  CircleDashed,
  RefreshCw,
  Clock3,
  X,
} from "lucide-react";
import {
  dependencyGraphKeys,
  dependencyGraphsOptions,
} from "@orvilo/core/dependency-graphs";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { useWorkspacePaths } from "@orvilo/core/paths";
import { useActorName } from "@orvilo/core/workspace/hooks";
import { useWSReconnect, useWSEvent } from "@orvilo/core/realtime";
import type {
  DependencyGraphNode,
  DependencyGraphResponse,
} from "@orvilo/core/types";
import { Button } from "@orvilo/ui/components/ui/button";
import { DependencyIcon } from "@orvilo/ui/components/common/dependency-icon";
import { cn } from "@orvilo/ui/lib/utils";
import {
  CollectionPageHeaderAction,
  CollectionPageState,
} from "../layout";
import { ShellHeaderActions } from "../layout/shell-header";
import { AppLink } from "../navigation";
import { useT } from "../i18n";
import { useStatusLabel } from "../issues/utils/status-label";
import { edgeEndpoint } from "./graph-utils";
import { GraphCanvas, type CanvasSelection } from "./graph-canvas";
import { GraphTaskStatus } from "./graph-task-status";
import { ViewStoreProvider } from "@orvilo/core/issues/stores/view-store-context";
import { getIssueSurfaceViewStore } from "@orvilo/core/issues/stores/surface-view-store";
import { useIssuesScope } from "@orvilo/core/issues/stores/issues-scope-store";
import { useActiveIssueView } from "@orvilo/core/issue-views/use-active-view";

const EMPTY_GRAPHS: DependencyGraphResponse[] = [];
function identifier(node: DependencyGraphNode) {
  return node.issue?.identifier || node.issue_id || node.temp_id || node.id;
}
function title(node: DependencyGraphNode) {
  return node.issue?.title || node.title || identifier(node);
}
function nodeID(node: DependencyGraphNode) {
  return node.id || node.temp_id || node.issue_id;
}
function edgeLabel(graph: DependencyGraphResponse, id: string) {
  const node = graph.nodes.find((n) =>
    [n.id, n.temp_id, n.issue_id, n.issue?.id, n.issue?.identifier].includes(
      id,
    ),
  );
  return node ? identifier(node) : id;
}

export function TaskGraphPage() {
  const wsId = useWorkspaceId();
  const scope = useIssuesScope("issues");
  const { activeView } = useActiveIssueView(wsId, { scope_type: "workspace" });
  const store = getIssueSurfaceViewStore(activeView ? `view:${activeView.id}` : `workspace:${scope}`);
  return <ViewStoreProvider store={store}><TaskGraphContent /></ViewStoreProvider>;
}

function TaskGraphContent() {
  const { t } = useT("task-graph");
  const wsId = useWorkspaceId();
  const paths = useWorkspacePaths();
  const client = useQueryClient();
  const labelOf = useStatusLabel(wsId);
  const { getActorName } = useActorName();
  const [selection, setSelection] = useState<CanvasSelection>(null);
  const query = useQuery({ ...dependencyGraphsOptions(wsId), enabled: !!wsId });
  const invalidate = useCallback(() => {
    void client.invalidateQueries({ queryKey: dependencyGraphKeys.all(wsId) });
  }, [client, wsId]);
  useWSEvent("dependency_graph:updated", invalidate);
  useWSEvent("issue:updated", invalidate);
  useWSEvent("issue:deleted", invalidate);
  useWSReconnect(invalidate);
  const graphs = query.data ?? EMPTY_GRAPHS;
  const selectedGraph = selection
    ? graphs.find((g) => g.plan.id === selection.planId)
    : undefined;
  const selectedNode =
    selection?.kind === "node"
      ? selectedGraph?.nodes.find((n) => nodeID(n) === selection.nodeId)
      : undefined;
  const selectedEdge =
    selection?.kind === "edge"
      ? selectedGraph?.edges.find((e) => e.id === selection.edgeId)
      : undefined;
  const statusLabel = (node: DependencyGraphNode) =>
    labelOf(node.issue?.status ?? node.status ?? "todo");
  const actorLabel = (node: DependencyGraphNode) => {
    const type = node.issue ? node.issue.executor_type : node.executor_type;
    const id = node.issue ? node.issue.executor_id : node.executor_id;
    return type && id ? getActorName(type, id) : t(($) => $.unassigned);
  };
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <ShellHeaderActions>
        <Button variant="ghost" size="sm" render={<AppLink href={paths.issues()} />} nativeButton={false}>
          <ArrowLeft />
          {t(($) => $.back_to_board)}
        </Button>
        <CollectionPageHeaderAction
          icon={RefreshCw}
          label={t(($) => $.refresh)}
          aria-label={t(($) => $.refresh)}
          onClick={() => void query.refetch()}
          disabled={query.isFetching}
        />
      </ShellHeaderActions>
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
            role="alert"
            actions={
              <Button variant="outline" onClick={() => void query.refetch()}>
                {t(($) => $.retry)}
              </Button>
            }
          />
        ) : graphs.length === 0 ? (
          <CollectionPageState
            icon={DependencyIcon}
            title={t(($) => $.empty_title)}
            description={t(($) => $.empty_hint)}
          />
        ) : (
          <div
            className={cn(
              "grid min-w-0 gap-5 p-4 lg:p-6",
              (selectedNode || selectedEdge) &&
                "xl:grid-cols-[minmax(0,1fr)_280px]",
            )}
          >
            <div className="min-w-0 space-y-6">
              {graphs.map((graph) => {
                const planTitle =
                  graph.parent?.title || graph.plan.goal || t(($) => $.plan);
                return (
                  <section
                    key={graph.plan.id}
                    aria-label={planTitle}
                    className="min-w-0"
                  >
                    {graph.plan.attention_required ? (
                      <div
                        className="mb-3 flex gap-2 rounded-md bg-muted p-3 text-caption"
                        role="status"
                      >
                        <AlertTriangle className="size-4 shrink-0" />
                        {t(($) => $.attention, {
                          reason:
                            graph.plan.attention_reason || t(($) => $.status),
                        })}
                      </div>
                    ) : null}
                    <GraphCanvas
                      graph={graph}
                      selection={selection}
                      onSelect={setSelection}
                      labels={{
                        canvas: t(($) => $.canvas, { plan: planTitle }),
                        nodeHint: (args) => t(($) => $.canvas_node, args),
                        edgeHint: ({ from, to, satisfied }) =>
                          t(($) => $.canvas_edge, {
                            from,
                            to,
                            status: satisfied
                              ? t(($) => $.satisfied)
                              : t(($) => $.blocked),
                          }),
                        status: statusLabel,
                        empty: t(($) => $.no_matching_tasks),
                        undrawn: (count) =>
                          t(($) => $.canvas_undrawn, { count }),
                      }}
                    />
                  </section>
                );
              })}
            </div>
            {selectedGraph && (selectedNode || selectedEdge) ? (
              <aside className="h-fit min-w-0 rounded-lg border border-border/60 p-4 xl:sticky xl:top-4">
                <div className="flex items-center justify-between gap-3">
                  <h2 className="text-body font-medium">
                    {t(($) => $.inspector)}
                  </h2>
                  <Button
                    variant="ghost"
                    size="icon-xs"
                    aria-label={t(($) => $.close_details)}
                    onClick={() => setSelection(null)}
                  >
                    <X />
                  </Button>
                </div>
                {selectedNode ? (
                  <div className="mt-4 space-y-4 text-caption">
                    <AppLink
                      href={paths.issueDetail(identifier(selectedNode))}
                      className="text-muted-foreground hover:underline"
                    >
                      {identifier(selectedNode)}
                    </AppLink>
                    <p className="text-body font-medium">
                      {title(selectedNode)}
                    </p>
                    <GraphTaskStatus
                      node={selectedNode}
                      label={statusLabel(selectedNode)}
                    />
                    <dl>
                      <dt className="text-muted-foreground">
                        {t(($) => $.executor)}
                      </dt>
                      <dd className="mt-1">{actorLabel(selectedNode)}</dd>
                    </dl>
                    {selectedNode.acceptance_criteria.length ? (
                      <ul className="list-disc space-y-1 pl-4 text-muted-foreground">
                        {selectedNode.acceptance_criteria.map((c, i) => (
                          <li key={i}>{c}</li>
                        ))}
                      </ul>
                    ) : null}
                    <AppLink
                      href={paths.issueDetail(identifier(selectedNode))}
                      className="inline-block underline underline-offset-4"
                    >
                      {t(($) => $.open_issue, {
                        identifier: identifier(selectedNode),
                      })}
                    </AppLink>
                  </div>
                ) : selectedEdge ? (
                  <dl className="mt-4 space-y-4 text-caption">
                    <div>
                      <dt className="text-muted-foreground">
                        {t(($) => $.dependency)}
                      </dt>
                      <dd className="mt-1">
                        {t(($) => $.dependency_from_to, {
                          from: edgeLabel(
                            selectedGraph,
                            edgeEndpoint(selectedEdge, "from"),
                          ),
                          to: edgeLabel(
                            selectedGraph,
                            edgeEndpoint(selectedEdge, "to"),
                          ),
                        })}
                      </dd>
                    </div>
                    <div>
                      <dt className="text-muted-foreground">
                        {t(($) => $.dependency_reason)}
                      </dt>
                      <dd className="mt-1">{selectedEdge.reason}</dd>
                    </div>
                    {selectedEdge.consumed_output ? (
                      <div>
                        <dt className="text-muted-foreground">
                          {t(($) => $.consumed_output)}
                        </dt>
                        <dd className="mt-1">{selectedEdge.consumed_output}</dd>
                      </div>
                    ) : null}
                    <p className="flex items-center gap-2">
                      {selectedEdge.satisfied ? (
                        <CheckCircle2 className="size-4" />
                      ) : (
                        <Clock3 className="size-4" />
                      )}
                      {selectedEdge.satisfied
                        ? t(($) => $.satisfied)
                        : t(($) => $.blocked)}
                    </p>
                  </dl>
                ) : null}
              </aside>
            ) : null}
          </div>
        )}
      </div>
    </div>
  );
}
