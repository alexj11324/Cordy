import { infiniteQueryOptions, queryOptions } from "@tanstack/react-query";
import { api } from "../api";
import type { WorkProductPageParams } from "../types";

export const WORK_PRODUCT_PAGE_SIZE = 64;

export const workProductKeys = {
  all: (wsId: string | null) => ["work-products", wsId] as const,
  list: (wsId: string | null, params: WorkProductPageParams = {}) =>
    [
      ...workProductKeys.all(wsId),
      "list",
      params.page ?? 1,
      params.per_page ?? WORK_PRODUCT_PAGE_SIZE,
    ] as const,
  detail: (wsId: string | null, id: string) =>
    [...workProductKeys.all(wsId), "detail", id] as const,
  // The issue's delivery list — products plus the relation that attached each
  // one. Kept under a root of its own so a realtime PR update can invalidate
  // every open issue's list without touching the workspace catalog.
  issueProductsRoot: (issueId: string) =>
    ["work-products", "issue", issueId] as const,
  taskProductsRoot: (wsId: string | null, taskId: string) =>
    [...workProductKeys.all(wsId), "task-products", taskId] as const,
  unassociated: (wsId: string | null, query = "", perPage = 20) =>
    [...workProductKeys.all(wsId), "unassociated", query, perPage] as const,
  provenanceRoot: (wsId: string | null) =>
    [...workProductKeys.all(wsId), "provenance"] as const,
  provenance: (wsId: string | null, params: WorkProductPageParams = {}) =>
    [
      ...workProductKeys.provenanceRoot(wsId),
      params.page ?? 1,
      params.per_page ?? WORK_PRODUCT_PAGE_SIZE,
    ] as const,
  taskProvenance: (wsId: string | null, taskId: string) =>
    [...workProductKeys.all(wsId), "task-provenance", taskId] as const,
};

function pageParams(params: WorkProductPageParams) {
  return {
    page: params.page ?? 1,
    per_page: params.per_page ?? WORK_PRODUCT_PAGE_SIZE,
  };
}

export function workProductListOptions(
  wsId: string | null,
  params: WorkProductPageParams & { enabled?: boolean } = {},
) {
  const { enabled = true, ...page } = params;
  return queryOptions({
    queryKey: workProductKeys.list(wsId, page),
    queryFn: ({ signal }) => api.listWorkProducts(pageParams(page), { signal }),
    enabled: !!wsId && enabled,
  });
}

export function workProductListInfiniteOptions(
  wsId: string | null,
  perPage = WORK_PRODUCT_PAGE_SIZE,
  enabled = true,
) {
  return infiniteQueryOptions({
    queryKey: [...workProductKeys.all(wsId), "list-infinite", perPage] as const,
    initialPageParam: 1,
    queryFn: ({ pageParam, signal }) =>
      api.listWorkProducts({ page: pageParam, per_page: perPage }, { signal }),
    getNextPageParam: (lastPage) =>
      lastPage.has_more ? lastPage.page + 1 : undefined,
    enabled: !!wsId && enabled,
  });
}

export function workProductDetailOptions(wsId: string | null, id: string) {
  return queryOptions({
    queryKey: workProductKeys.detail(wsId, id),
    queryFn: ({ signal }) => api.getWorkProduct(id, { signal }),
    enabled: !!wsId && !!id,
  });
}

export function unassociatedWorkProductListInfiniteOptions(
  wsId: string | null,
  perPage = 20,
  query = "",
  enabled = true,
) {
  return infiniteQueryOptions({
    queryKey: workProductKeys.unassociated(wsId, query, perPage),
    initialPageParam: 1,
    queryFn: ({ pageParam, signal }) =>
      api.listUnassociatedWorkProducts({ page: pageParam, per_page: perPage, query }, { signal }),
    getNextPageParam: (lastPage) => lastPage.next_page ?? undefined,
    enabled: !!wsId && enabled,
  });
}

export function workProductProvenanceOptions(
  wsId: string | null,
  params: WorkProductPageParams = {},
) {
  return queryOptions({
    queryKey: workProductKeys.provenance(wsId, params),
    queryFn: ({ signal }) =>
      api.listWorkspaceProvenance(pageParams(params), { signal }),
    enabled: !!wsId,
  });
}

export function workProductProvenanceInfiniteOptions(
  wsId: string | null,
  perPage = WORK_PRODUCT_PAGE_SIZE,
) {
  return infiniteQueryOptions({
    queryKey: [...workProductKeys.provenanceRoot(wsId), "infinite", perPage] as const,
    initialPageParam: 1,
    queryFn: ({ pageParam, signal }) =>
      api.listWorkspaceProvenance({ page: pageParam, per_page: perPage }, { signal }),
    getNextPageParam: (lastPage) =>
      lastPage.has_more ? lastPage.page + 1 : undefined,
    enabled: !!wsId,
  });
}

export function taskProvenanceOptions(wsId: string | null, taskId: string) {
  return queryOptions({
    queryKey: workProductKeys.taskProvenance(wsId, taskId),
    queryFn: ({ signal }) => api.getTaskProvenance(taskId, { signal }),
    enabled: !!wsId && !!taskId,
  });
}

export function issueWorkProductsOptions(
  wsId: string | null,
  issueId: string,
) {
  return queryOptions({
    queryKey: workProductKeys.issueProductsRoot(issueId),
    queryFn: ({ signal }) => api.listIssueWorkProducts(issueId, { signal }),
    enabled: !!wsId && !!issueId,
  });
}

export function issueWorkProductsInfiniteOptions(
  wsId: string | null,
  issueId: string,
  perPage = WORK_PRODUCT_PAGE_SIZE,
) {
  return infiniteQueryOptions({
    queryKey: [...workProductKeys.issueProductsRoot(issueId), "infinite", perPage] as const,
    initialPageParam: 1,
    queryFn: ({ signal }) => api.listIssueWorkProducts(issueId, { signal }),
    getNextPageParam: () => undefined,
    enabled: !!wsId && !!issueId,
  });
}

export function taskWorkProductsOptions(
  wsId: string | null,
  taskId: string,
) {
  return queryOptions({
    queryKey: workProductKeys.taskProductsRoot(wsId, taskId),
    queryFn: ({ signal }) => api.listTaskWorkProducts(taskId, { signal }),
    enabled: !!wsId && !!taskId,
  });
}
