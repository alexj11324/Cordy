import { queryOptions } from "@tanstack/react-query";
import { api, ApiError } from "../api";
import type {
  DependencyGraphResponse,
  ListDependencyGraphsResponse,
} from "../types";

export const dependencyGraphKeys = {
  all: (wsId: string) => ["dependency-graphs", wsId] as const,
  list: (wsId: string, projectId?: string) =>
    [...dependencyGraphKeys.all(wsId), "list", projectId ?? null] as const,
  detail: (wsId: string, issueId: string) =>
    [...dependencyGraphKeys.all(wsId), "detail", issueId] as const,
};

async function loadAllDependencyGraphs(
  projectId: string | undefined,
  signal: AbortSignal | undefined,
): Promise<DependencyGraphResponse[]> {
  const graphs: DependencyGraphResponse[] = [];
  let cursor: string | undefined;

  for (;;) {
    const page: ListDependencyGraphsResponse = await api.listDependencyGraphs(
      {
        ...(projectId ? { projectId } : {}),
        limit: 64,
        ...(cursor ? { cursor } : {}),
      },
      { signal },
    );
    graphs.push(...page.graphs);

    const nextCursor = page.next_cursor ?? undefined;
    if (!nextCursor) break;
    if (nextCursor === cursor) {
      throw new Error("Dependency graph pagination returned a repeated cursor");
    }
    cursor = nextCursor;
  }

  return graphs;
}

export function dependencyGraphsOptions(wsId: string, projectId?: string) {
  return queryOptions({
    queryKey: dependencyGraphKeys.list(wsId, projectId),
    queryFn: ({ signal }) => loadAllDependencyGraphs(projectId, signal),
    staleTime: 0,
  });
}

export function dependencyGraphOptions(wsId: string, issueId: string) {
  return queryOptions<DependencyGraphResponse | null>({
    queryKey: dependencyGraphKeys.detail(wsId, issueId),
    queryFn: async ({ signal }) => {
      try {
        return await api.getDependencyGraph(issueId, { signal });
      } catch (error) {
        // An issue without an active graph is a normal state. Preserve real
        // transport/auth/server failures for the detail surface to report.
        if (error instanceof ApiError && error.status === 404) return null;
        throw error;
      }
    },
    staleTime: 0,
  });
}
