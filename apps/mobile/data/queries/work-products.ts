/**
 * Mobile-owned Work Product queries. The mobile API wrapper is intentionally
 * separate from the browser client (auth, timeout, and workspace header
 * behavior differ), but cache identity and response types mirror core.
 */
import { queryOptions } from "@tanstack/react-query";
import { api } from "@/data/api";

export const workProductKeys = {
  all: (wsId: string | null) => ["work-products", wsId] as const,
  list: (wsId: string | null) => [...workProductKeys.all(wsId), "list"] as const,
  detail: (wsId: string | null, id: string) =>
    [...workProductKeys.all(wsId), "detail", id] as const,
};

export const workProductListOptions = (wsId: string | null) =>
  queryOptions({
    queryKey: workProductKeys.list(wsId),
    queryFn: async ({ signal }) => {
      const response = await api.listWorkProducts({}, { signal });
      return response.products;
    },
    enabled: !!wsId,
  });

export const workProductDetailOptions = (wsId: string | null, id: string) =>
  queryOptions({
    queryKey: workProductKeys.detail(wsId, id),
    queryFn: ({ signal }) => api.getWorkProduct(id, { signal }),
    enabled: !!wsId && !!id,
  });
