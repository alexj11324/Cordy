import type { IssueSurfaceQueryPlan } from "./query-plan";
import { projectGanttIssuesOptions } from "../queries";

/**
 * Issue surface repository — resolves a {@link IssueSurfaceQueryPlan} to
 * concrete query options. Every list-shaped mode (table, list, board,
 * swimlane) is served by the server-owned Table query channel; the Gantt
 * projection below is the one remaining scoped fetch, and it compiles its
 * owner/executor narrowing from the same plan as everything else.
 */
export function issueSurfaceGanttOptions(
  wsId: string,
  projectId: string,
  plan: IssueSurfaceQueryPlan,
) {
  return projectGanttIssuesOptions(
    wsId,
    projectId,
    {
      ...(plan.queryFilter.owner_types
        ? { owner_types: plan.queryFilter.owner_types }
        : {}),
      ...(plan.queryFilter.executor_types
        ? { executor_types: plan.queryFilter.executor_types }
        : {}),
    },
  );
}
