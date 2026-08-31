import { queryOptions } from "@tanstack/react-query";
import { api } from "../api";

export const linearKeys = {
  all: (workspaceId: string) => ["linear", workspaceId] as const,
  connection: (workspaceId: string) =>
    [...linearKeys.all(workspaceId), "connection"] as const,
  catalog: (workspaceId: string) =>
    [...linearKeys.all(workspaceId), "catalog"] as const,
  bindings: (workspaceId: string) =>
    [...linearKeys.all(workspaceId), "bindings"] as const,
  memberBindings: (workspaceId: string) =>
    [...linearKeys.all(workspaceId), "member-bindings"] as const,
  conflicts: (workspaceId: string) =>
    [...linearKeys.all(workspaceId), "conflicts"] as const,
};

export const linearConnectionOptions = (workspaceId: string) =>
  queryOptions({
    queryKey: linearKeys.connection(workspaceId),
    queryFn: () => api.getLinearConnection(workspaceId),
    enabled: !!workspaceId,
  });

export const linearCatalogOptions = (workspaceId: string, enabled = true) =>
  queryOptions({
    queryKey: linearKeys.catalog(workspaceId),
    queryFn: () => api.getLinearCatalog(workspaceId),
    enabled: enabled && !!workspaceId,
  });

export const linearBindingsOptions = (workspaceId: string) =>
  queryOptions({
    queryKey: linearKeys.bindings(workspaceId),
    queryFn: () => api.listLinearBindings(workspaceId),
    enabled: !!workspaceId,
  });

export const linearMemberBindingsOptions = (workspaceId: string) =>
  queryOptions({
    queryKey: linearKeys.memberBindings(workspaceId),
    queryFn: () => api.listLinearMemberBindings(workspaceId),
    enabled: !!workspaceId,
  });

export const linearConflictsOptions = (workspaceId: string) =>
  queryOptions({
    queryKey: linearKeys.conflicts(workspaceId),
    queryFn: () => api.listLinearSyncConflicts(workspaceId),
    enabled: !!workspaceId,
  });
