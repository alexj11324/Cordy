"use client";

import { useCallback, useMemo } from "react";
import { Check, CircleAlert, LockKeyhole, LoaderCircle } from "lucide-react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { dependencyGraphKeys, dependencyGraphOptions } from "@patchbay/core/dependency-graphs";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { useWSReconnect, useWSEvent } from "@patchbay/core/realtime";
import type {
  DependencyGraphEdge,
  DependencyGraphNode,
  DependencyGraphResponse,
} from "@patchbay/core/types";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { Button } from "@patchbay/ui/components/ui/button";
import { cn } from "@patchbay/ui/lib/utils";
import { AppLink } from "../../navigation";
import { useStatusLabel } from "../utils/status-label";
import { useT } from "../../i18n";
import { CustomStatusChip } from "./custom-status-chip";
import { StatusIcon } from "./status-icon";

export type DependencyPrerequisite = {
  edge: DependencyGraphEdge;
  node: DependencyGraphNode;
};

export type DependencyPrerequisiteBlockKind = "cancelled" | "attention" | "waiting";

const READINESS_KEYS: Record<string, "todo" | "ready" | "running" | "blocked" | "done" | "cancelled"> = {
  todo: "todo",
  ready: "ready",
  running: "running",
  blocked: "blocked",
  done: "done",
  cancelled: "cancelled",
};

/**
 * Selects persisted incoming hard edges for one issue. This is intentionally
 * pure so the detail contract can be regression-tested without replacing the
 * real React Query/API path with fixture-only state.
 */
export function selectDependencyPrerequisites(
  graph: DependencyGraphResponse,
  issueId: string,
): DependencyPrerequisite[] {
  const nodesByIssueId = new Map(graph.nodes.map((node) => [node.issue_id, node]));
  return graph.edges
    .filter((edge) => edge.type === "hard" && edge.to_issue_id === issueId)
    .flatMap((edge) => {
      const node = nodesByIssueId.get(edge.from_issue_id);
      return node ? [{ edge, node }] : [];
    })
    .sort((left, right) => {
      const leftIdentifier = left.node.issue.identifier || left.node.issue_id;
      const rightIdentifier = right.node.issue.identifier || right.node.issue_id;
      return leftIdentifier.localeCompare(rightIdentifier);
    });
}

function targetNode(graph: DependencyGraphResponse, issueId: string): DependencyGraphNode | undefined {
  return graph.nodes.find((node) => node.issue_id === issueId);
}

export function dependencyPrerequisiteBlockKind(
  graph: DependencyGraphResponse,
  prerequisite: DependencyPrerequisite,
): DependencyPrerequisiteBlockKind | null {
  if (prerequisite.edge.satisfied) return null;
  if (prerequisite.edge.prerequisite_status === "cancelled") {
    return "cancelled";
  }
  if (graph.plan.attention_required && graph.plan.attention_reason) {
    return "attention";
  }
  return "waiting";
}

function prerequisiteBlockReason(
  graph: DependencyGraphResponse,
  prerequisite: DependencyPrerequisite,
  t: ReturnType<typeof useT<"issues">>["t"],
): string | null {
  const kind = dependencyPrerequisiteBlockKind(graph, prerequisite);
  if (kind === null) return null;
  if (kind === "cancelled") return t(($) => $.detail.dependency_cancelled);
  if (kind === "attention") {
    return t(($) => $.detail.dependency_failed, {
      reason: graph.plan.attention_reason ?? "",
    });
  }
  return t(($) => $.detail.dependency_waiting);
}

function DependencyPrerequisiteRow({
  graph,
  prerequisite,
  statusLabel,
}: {
  graph: DependencyGraphResponse;
  prerequisite: DependencyPrerequisite;
  statusLabel: (status: string) => string;
}) {
  const { t } = useT("issues");
  const paths = useWorkspacePaths();
  const source = prerequisite.node;
  const satisfied = prerequisite.edge.satisfied;
  const reason = prerequisiteBlockReason(graph, prerequisite, t);
  const sourceIdentifier = source.issue.identifier || source.issue.id;
  const sourceStatusLabel = statusLabel(source.issue.status);

  return (
    <li
      className={cn(
        "rounded-md border px-2.5 py-2",
        satisfied
          ? "border-emerald-500/20 bg-emerald-500/5"
          : "border-amber-500/25 bg-amber-500/5",
      )}
    >
      <div className="flex min-w-0 items-start gap-2">
        {satisfied ? (
          <Check
            className="mt-0.5 size-4 shrink-0 text-emerald-600 dark:text-emerald-400"
            aria-label={t(($) => $.detail.dependency_done)}
          />
        ) : (
          <StatusIcon
            status={source.issue.status}
            className="mt-0.5 size-4 shrink-0"
          />
        )}
        <div className="min-w-0 flex-1">
          <AppLink
            href={paths.issueDetail(sourceIdentifier)}
            className="flex min-w-0 items-center gap-1.5 text-caption font-medium hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60"
            aria-label={t(($) => $.detail.dependency_open_prerequisite, {
              identifier: sourceIdentifier,
            })}
          >
            <span className="shrink-0 text-muted-foreground">{sourceIdentifier}</span>
            <span className="truncate">{source.title}</span>
          </AppLink>
          <div className="mt-1 flex min-w-0 flex-wrap items-center gap-1.5 text-micro text-muted-foreground">
            <span className={cn(satisfied && "text-emerald-700 dark:text-emerald-300")}>
              {sourceStatusLabel}
            </span>
            <CustomStatusChip status={source.issue.status} />
            <span aria-hidden>·</span>
            <span>
              {satisfied
                ? t(($) => $.detail.dependency_done)
                : t(($) => $.detail.dependency_pending)}
            </span>
          </div>
          <p className="mt-1 text-micro text-muted-foreground">
            {prerequisite.edge.reason}
          </p>
          {reason && (
            <p
              role="status"
              className={cn(
                "mt-1 flex items-start gap-1 text-micro",
                prerequisite.edge.prerequisite_status === "cancelled"
                  ? "text-destructive"
                  : "text-amber-700 dark:text-amber-300",
              )}
            >
              <CircleAlert className="mt-0.5 size-3 shrink-0" aria-hidden="true" />
              <span>{reason}</span>
            </p>
          )}
        </div>
      </div>
    </li>
  );
}

/**
 * Detail-side explanation of the same persisted gate used by scheduler
 * admission. The 404/empty response is a real no-dependency state; all other
 * API errors remain visible so an unavailable graph cannot look unblocked.
 */
export function DependencyPrerequisites({ issueId }: { issueId: string }) {
  const { t } = useT("issues");
  const wsId = useWorkspaceId();
  const queryClient = useQueryClient();
  const statusLabel = useStatusLabel(wsId);
  const query = useQuery({
    ...dependencyGraphOptions(wsId, issueId),
    enabled: wsId.length > 0 && issueId.length > 0,
  });

  const invalidate = useCallback(() => {
    if (!wsId) return;
    void queryClient.invalidateQueries({ queryKey: dependencyGraphKeys.all(wsId) });
  }, [queryClient, wsId]);
  useWSEvent("dependency_graph:updated", invalidate);
  useWSReconnect(invalidate);

  const graph = query.data;
  const prerequisites = useMemo(
    () => (graph ? selectDependencyPrerequisites(graph, issueId) : []),
    [graph, issueId],
  );
  const node = graph ? targetNode(graph, issueId) : undefined;
  const satisfied = node?.readiness.satisfied_prerequisites ?? 0;
  const total = node?.readiness.total_prerequisites ?? prerequisites.length;
  const stateLabel = node
    ? t(($) => $.graph.readiness_state[READINESS_KEYS[node.readiness.state] ?? "todo"])
    : null;

  if (query.isPending) {
    return (
      <section aria-labelledby="dependency-prerequisites-heading" className="space-y-2">
        <h2
          id="dependency-prerequisites-heading"
          className="flex items-center gap-1.5 px-2 text-caption font-medium"
        >
          <LockKeyhole className="size-3.5 text-muted-foreground" aria-hidden="true" />
          {t(($) => $.detail.section_dependencies)}
        </h2>
        <div className="flex items-center gap-2 px-2 text-micro text-muted-foreground" role="status">
          <LoaderCircle className="size-3 animate-spin" aria-hidden="true" />
          {t(($) => $.detail.dependency_loading)}
        </div>
      </section>
    );
  }

  if (query.isError) {
    return (
      <section aria-labelledby="dependency-prerequisites-heading" className="space-y-2">
        <h2
          id="dependency-prerequisites-heading"
          className="flex items-center gap-1.5 px-2 text-caption font-medium"
        >
          <LockKeyhole className="size-3.5 text-muted-foreground" aria-hidden="true" />
          {t(($) => $.detail.section_dependencies)}
        </h2>
        <div className="rounded-md border border-destructive/25 bg-destructive/5 px-2.5 py-2 text-micro text-destructive" role="alert">
          <p>{t(($) => $.detail.dependency_load_failed)}</p>
          <Button
            variant="ghost"
            size="sm"
            className="mt-1 h-6 px-1.5 text-micro"
            onClick={() => void query.refetch()}
          >
            {t(($) => $.detail.dependency_retry)}
          </Button>
        </div>
      </section>
    );
  }

  return (
    <section aria-labelledby="dependency-prerequisites-heading" className="space-y-2">
      <h2
        id="dependency-prerequisites-heading"
        className="flex items-center gap-1.5 px-2 text-caption font-medium"
      >
        <LockKeyhole className="size-3.5 text-muted-foreground" aria-hidden="true" />
        {t(($) => $.detail.section_dependencies)}
      </h2>
      {node && stateLabel && (
        <p
          className={cn(
            "px-2 text-micro tabular-nums",
            node.readiness.gate_open
              ? "text-emerald-700 dark:text-emerald-300"
              : "text-amber-700 dark:text-amber-300",
          )}
          role="status"
          aria-live="polite"
        >
          {stateLabel} · {satisfied}/{total} {t(($) => $.detail.dependency_count_suffix)}
        </p>
      )}
      {prerequisites.length === 0 ? (
        <p className="px-2 text-micro text-muted-foreground" role="status">
          {t(($) => $.detail.dependency_no_prerequisites)}
        </p>
      ) : (
        <ul className="space-y-2" aria-label={t(($) => $.detail.section_dependencies)}>
          {prerequisites.map((prerequisite) => (
            <DependencyPrerequisiteRow
              key={prerequisite.edge.id}
              graph={graph!}
              prerequisite={prerequisite}
              statusLabel={statusLabel}
            />
          ))}
        </ul>
      )}
      {graph?.plan.attention_required && graph.plan.attention_reason && prerequisites.length > 0 && (
        <p className="flex items-start gap-1.5 rounded-md bg-amber-500/10 px-2.5 py-2 text-micro text-amber-800 dark:text-amber-200" role="alert">
          <CircleAlert className="mt-0.5 size-3 shrink-0" aria-hidden="true" />
          <span>{t(($) => $.detail.dependency_attention, { reason: graph.plan.attention_reason })}</span>
        </p>
      )}
    </section>
  );
}
