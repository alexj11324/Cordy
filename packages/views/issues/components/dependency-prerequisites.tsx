"use client";

import { useCallback, useMemo } from "react";
import { Check, CircleAlert, LoaderCircle, LockKeyhole } from "lucide-react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  dependencyGraphKeys,
  dependencyGraphOptions,
  selectDependencyPrerequisiteState,
  type DependencyPrerequisite,
} from "@patchbay/core/dependency-graphs";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { useWSReconnect, useWSEvent } from "@patchbay/core/realtime";
import type { DependencyGraphResponse } from "@patchbay/core/types";
import { Button } from "@patchbay/ui/components/ui/button";
import { cn } from "@patchbay/ui/lib/utils";
import { useT } from "../../i18n";
import { AppLink } from "../../navigation";
import { useStatusLabel } from "../utils/status-label";
import { StatusIcon } from "./status-icon";

function blockerReason(
  graph: DependencyGraphResponse,
  prerequisite: DependencyPrerequisite,
  t: ReturnType<typeof useT<"issues">>["t"],
): string | null {
  if (prerequisite.satisfied) return null;
  if (prerequisite.node.status_category === "cancelled") {
    return t(($) => $.detail.dependency_cancelled);
  }
  if (graph.plan.attention_required && graph.plan.attention_reason) {
    return t(($) => $.detail.dependency_attention, {
      reason: graph.plan.attention_reason,
    });
  }
  return t(($) => $.detail.dependency_waiting);
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function issueEventValue(payload: unknown, key: "id" | "parent_issue_id"): string | null {
  const record = asRecord(payload);
  const issue = asRecord(record?.issue);
  const value = issue?.[key] ?? record?.[key];
  return typeof value === "string" && value ? value : null;
}

function issueEventID(payload: unknown): string | null {
  const record = asRecord(payload);
  const issueID = issueEventValue(payload, "id") ?? record?.issue_id;
  return typeof issueID === "string" && issueID ? issueID : null;
}

function PrerequisiteRow({
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
  const reason = blockerReason(graph, prerequisite, t);

  return (
    <li
      className={cn(
        "rounded-md border px-2.5 py-2",
        prerequisite.satisfied
          ? "border-emerald-500/20 bg-emerald-500/5"
          : "border-amber-500/25 bg-amber-500/5",
      )}
    >
      <div className="flex min-w-0 items-start gap-2">
        {prerequisite.satisfied ? (
          <Check
            aria-label={t(($) => $.detail.dependency_done)}
            className="mt-0.5 size-4 shrink-0 text-emerald-600 dark:text-emerald-400"
          />
        ) : (
          <StatusIcon status={source.status} className="mt-0.5 size-4" />
        )}
        <div className="min-w-0 flex-1">
          <AppLink
            href={paths.issueDetail(source.issue_id)}
            className="block truncate text-caption font-medium hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60"
            aria-label={t(($) => $.detail.dependency_open_prerequisite, {
              title: source.title,
            })}
          >
            {source.title}
          </AppLink>
          <p className="mt-1 text-micro text-muted-foreground">
            {statusLabel(source.status)} · {prerequisite.satisfied
              ? t(($) => $.detail.dependency_done)
              : t(($) => $.detail.dependency_pending)}
          </p>
          {prerequisite.edge.reason && (
            <p className="mt-1 text-micro text-muted-foreground">
              {prerequisite.edge.reason}
            </p>
          )}
          {reason && (
            <p role="status" className="mt-1 flex items-start gap-1 text-micro text-amber-700 dark:text-amber-300">
              <CircleAlert aria-hidden="true" className="mt-0.5 size-3 shrink-0" />
              <span>{reason}</span>
            </p>
          )}
        </div>
      </div>
    </li>
  );
}

export function DependencyPrerequisites({ issueId }: { issueId: string }) {
  const { t } = useT("issues");
  const wsId = useWorkspaceId();
  const queryClient = useQueryClient();
  const statusLabel = useStatusLabel(wsId);
  const query = useQuery({
    ...dependencyGraphOptions(wsId, issueId),
    enabled: Boolean(wsId && issueId),
  });
  const invalidate = useCallback(() => {
    if (wsId) {
      void queryClient.invalidateQueries({ queryKey: dependencyGraphKeys.all(wsId) });
    }
  }, [queryClient, wsId]);
  useWSEvent("dependency_graph:updated", invalidate);
  useWSReconnect(invalidate);

  const state = useMemo(
    () => query.data
      ? selectDependencyPrerequisiteState(query.data, issueId)
      : null,
    [issueId, query.data],
  );
  const graphNodeIDs = useMemo(
    () => new Set(query.data?.nodes.map((node) => node.issue_id) ?? []),
    [query.data],
  );
  const refreshForIssueUpdate = useCallback((payload: unknown) => {
    const changedIssueID = issueEventID(payload);
    if (changedIssueID === issueId || (changedIssueID && graphNodeIDs.has(changedIssueID))) {
      invalidate();
    }
  }, [graphNodeIDs, invalidate, issueId]);
  const refreshForGraphIssueCreation = useCallback((payload: unknown) => {
    const record = asRecord(payload);
    if (
      typeof record?.dependency_graph_plan_id === "string" &&
      issueEventValue(payload, "parent_issue_id") === issueId
    ) {
      invalidate();
    }
  }, [invalidate, issueId]);
  const refreshForIssueDeletion = useCallback((payload: unknown) => {
    const deletedIssueID = issueEventID(payload);
    if (deletedIssueID === issueId || (deletedIssueID && graphNodeIDs.has(deletedIssueID))) {
      invalidate();
    }
  }, [graphNodeIDs, invalidate, issueId]);
  // The Go API currently emits issue lifecycle events, not a dedicated
  // dependency_graph:updated frame. Refresh on the real events that can
  // create, complete, or remove a graph issue; retain the dedicated event
  // subscription for newer servers that add it.
  useWSEvent("issue:created", refreshForGraphIssueCreation);
  useWSEvent("issue:updated", refreshForIssueUpdate);
  useWSEvent("issue:deleted", refreshForIssueDeletion);

  return (
    <section aria-labelledby="dependency-prerequisites-heading" className="space-y-2">
      <h2 id="dependency-prerequisites-heading" className="flex items-center gap-1.5 px-2 text-caption font-medium">
        <LockKeyhole aria-hidden="true" className="size-3.5 text-muted-foreground" />
        {t(($) => $.detail.section_dependencies)}
      </h2>
      {query.isPending ? (
        <p role="status" className="flex items-center gap-2 px-2 text-micro text-muted-foreground">
          <LoaderCircle aria-hidden="true" className="size-3 animate-spin" />
          {t(($) => $.detail.dependency_loading)}
        </p>
      ) : query.isError ? (
        <div role="alert" className="rounded-md border border-destructive/25 bg-destructive/5 px-2.5 py-2 text-micro text-destructive">
          <p>{t(($) => $.detail.dependency_load_failed)}</p>
          <Button variant="ghost" size="sm" className="mt-1 h-6 px-1.5 text-micro" onClick={() => void query.refetch()}>
            {t(($) => $.detail.dependency_retry)}
          </Button>
        </div>
      ) : !state || state.total === 0 ? (
        <p className="px-2 text-micro text-muted-foreground">
          {t(($) => $.detail.dependency_no_prerequisites)}
        </p>
      ) : (
        <>
          <p
            role="status"
            aria-live="polite"
            className={cn(
              "px-2 text-micro tabular-nums",
              state.ready
                ? "text-emerald-700 dark:text-emerald-300"
                : "text-amber-700 dark:text-amber-300",
            )}
          >
            {state.ready
              ? t(($) => $.detail.dependency_ready)
              : t(($) => $.detail.dependency_blocked, { count: state.blockedBy.length })}
            {" · "}{t(($) => $.detail.dependency_summary, {
              satisfied: state.satisfied,
              total: state.total,
            })}
          </p>
          <ul className="space-y-1.5">
            {state.prerequisites.map((prerequisite) => (
              <PrerequisiteRow
                key={prerequisite.edge.id}
                graph={query.data!}
                prerequisite={prerequisite}
                statusLabel={statusLabel}
              />
            ))}
          </ul>
        </>
      )}
    </section>
  );
}
