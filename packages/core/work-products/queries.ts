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
  relationsRoot: (wsId: string | null, issueId: string) =>
    [...workProductKeys.all(wsId), "relations", issueId] as const,
  relations: (
    wsId: string | null,
    issueId: string,
    params: WorkProductPageParams = {},
  ) =>
    [
      ...workProductKeys.relationsRoot(wsId, issueId),
      params.page ?? 1,
      params.per_page ?? WORK_PRODUCT_PAGE_SIZE,
    ] as const,
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

export function workProductRelationsOptions(
  wsId: string | null,
  issueId: string,
  params: WorkProductPageParams = {},
) {
  return queryOptions({
    queryKey: workProductKeys.relations(wsId, issueId, params),
    queryFn: ({ signal }) =>
      api.listWorkProductRelations(issueId, pageParams(params), { signal }),
    enabled: !!wsId && !!issueId,
  });
}

export function workProductRelationsInfiniteOptions(
  wsId: string | null,
  issueId: string,
  perPage = WORK_PRODUCT_PAGE_SIZE,
) {
  return infiniteQueryOptions({
    queryKey: [...workProductKeys.relationsRoot(wsId, issueId), "infinite", perPage] as const,
    initialPageParam: 1,
    queryFn: ({ pageParam, signal }) =>
      api.listWorkProductRelations(issueId, { page: pageParam, per_page: perPage }, { signal }),
    getNextPageParam: (lastPage) =>
      lastPage.has_more ? lastPage.page + 1 : undefined,
    enabled: !!wsId && !!issueId,
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
