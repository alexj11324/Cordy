"use client";

import { useMemo } from "react";
import { LockKeyhole } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { api } from "@patchbay/core/api";
import { dependencyGraphsOptions } from "@patchbay/core/dependency-graphs";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { cn } from "@patchbay/ui/lib/utils";
import { useT } from "../../i18n";

/**
 * Read the same persisted graph projection used by Graph view and expose only
 * unsatisfied prerequisites for one issue. This deliberately does not infer
 * blockers from stage/parent state: the dependency graph API is the source of
 * truth for semantic dependencies.
 */
function useDependencyBlockers(issueId: string): string[] {
  const wsId = useWorkspaceId();
  // Keep isolated component tests and pre-provider renders inert when the
  // singleton has not been initialized yet. The production provider installs
  // the full ApiClient before issue surfaces mount, so real blocker data still
  // always comes from the persisted graph endpoint.
  const graphApiAvailable = typeof api.listDependencyGraphs === "function";
  const query = useQuery({
    ...dependencyGraphsOptions(wsId),
    enabled: graphApiAvailable && wsId.length > 0 && issueId.length > 0,
  });

  return useMemo(() => {
    const identifiers = new Set<string>();
    for (const graph of query.data ?? []) {
      const nodesByIssueId = new Map(
        graph.nodes.map((node) => [node.issue_id, node.issue.identifier || node.issue_id]),
      );
      for (const edge of graph.edges) {
        if (edge.to_issue_id !== issueId || edge.satisfied) continue;
        const identifier = nodesByIssueId.get(edge.from_issue_id);
        if (identifier) identifiers.add(identifier);
      }
    }
    return Array.from(identifiers).sort((left, right) => left.localeCompare(right));
  }, [issueId, query.data]);
}

export function DependencyBlockerBadge({
  issueId,
  className,
}: {
  issueId: string;
  className?: string;
}) {
  const { t } = useT("issues");
  const blockers = useDependencyBlockers(issueId);
  if (blockers.length === 0) return null;

  const label = t(($) => $.graph.blocked_by, { identifiers: blockers.join(", ") });
  return (
    <span
      role="status"
      aria-label={label}
      title={label}
      className={cn(
        "inline-flex max-w-full items-center gap-1 rounded-full bg-amber-500/10 px-1.5 py-0.5 text-micro text-amber-700 dark:text-amber-300",
        className,
      )}
    >
      <LockKeyhole className="size-3 shrink-0" aria-hidden="true" />
      <span className="truncate">{label}</span>
    </span>
  );
}
