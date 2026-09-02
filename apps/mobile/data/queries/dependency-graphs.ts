import { queryOptions } from "@tanstack/react-query";
import type {
  DependencyGraphResponse,
  ListDependencyGraphsResponse,
} from "@patchbay/core/types";
import { api } from "@/data/api";

export const dependencyGraphKeys = {
  all: (wsId: string | null) => ["dependency-graphs", wsId] as const,
  list: (wsId: string | null, projectId?: string) =>
    [...dependencyGraphKeys.all(wsId), "list", projectId ?? null] as const,
  detail: (wsId: string | null, issueId: string) =>
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

export const dependencyGraphsOptions = (
  wsId: string | null,
  projectId?: string,
) =>
  queryOptions({
    queryKey: dependencyGraphKeys.list(wsId, projectId),
    queryFn: ({ signal }) => loadAllDependencyGraphs(projectId, signal),
    enabled: !!wsId,
  });

export const dependencyGraphOptions = (wsId: string | null, issueId: string) =>
  queryOptions({
    queryKey: dependencyGraphKeys.detail(wsId, issueId),
    queryFn: ({ signal }) => api.getDependencyGraph(issueId, { signal }),
    enabled: !!wsId && !!issueId,
  });
