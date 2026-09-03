import { infiniteQueryOptions, queryOptions, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api";
import type { ListDingTalkGroupRoutesResponse } from "../types/dingtalk";

/** Query key namespace for everything DingTalk-installation-related. Realtime
 * sync invalidates `installations(wsId)` on `dingtalk_installation:*` events so
 * the Settings panel updates without a manual refetch (e.g. after a binding
 * lands the install in another tab). */
export const dingtalkKeys = {
  all: (wsId: string) => ["dingtalk", wsId] as const,
  installations: (wsId: string) => [...dingtalkKeys.all(wsId), "installations"] as const,
  groups: (wsId: string) => [...dingtalkKeys.all(wsId), "groups"] as const,
  groupRoutes: (wsId: string) => [...dingtalkKeys.all(wsId), "group-routes"] as const,
  agentGroups: (wsId: string, agentId: string) =>
    [...dingtalkKeys.groups(wsId), "agent", agentId] as const,
  inactiveGroups: (wsId: string, installationId: string) =>
    [...dingtalkKeys.groups(wsId), "inactive", installationId] as const,
  agentInactiveGroups: (wsId: string, agentId: string, installationId: string) =>
    [...dingtalkKeys.agentGroups(wsId, agentId), "inactive", installationId] as const,
};

export const dingtalkGroupRoutesOptions = (wsId: string) =>
  queryOptions({
    queryKey: dingtalkKeys.groupRoutes(wsId),
    queryFn: () => api.listDingTalkGroupRoutes(wsId),
    enabled: !!wsId,
    refetchInterval: (query) => query.state.status === "success" ? 5_000 : false,
  });

export function useUpdateDingTalkGroupRoute(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async ({ routeId, agentId }: { routeId: string; agentId: string }) => {
      const updated = await api.updateDingTalkGroupRoute(wsId, routeId, { agent_id: agentId });
      // A schema fallback is not a successful reassignment. Keep the old
      // selection until the server confirms the requested resource and target.
      if (updated.id !== routeId || updated.agent_id !== agentId) {
        throw new Error("Invalid DingTalk group-route update response");
      }
      return updated;
    },
    onSuccess: (updated) => {
      qc.setQueryData<ListDingTalkGroupRoutesResponse>(dingtalkKeys.groupRoutes(wsId), (current) =>
        current ? { ...current, routes: current.routes.map((route) => route.id === updated.id ? updated : route) } : current,
      );
    },
    onSettled: () => qc.invalidateQueries({ queryKey: dingtalkKeys.groupRoutes(wsId) }),
  });
}

export const dingtalkInstallationsOptions = (wsId: string) =>
  queryOptions({
    queryKey: dingtalkKeys.installations(wsId),
    queryFn: () => api.listDingTalkInstallations(wsId),
    enabled: !!wsId,
  });

export const dingtalkGroupsOptions = (wsId: string) =>
  queryOptions({
    queryKey: dingtalkKeys.groups(wsId),
    queryFn: () => api.listDingTalkGroups(wsId),
    enabled: !!wsId,
    // Group discovery arrives through DingTalk Stream callbacks rather than an
    // HTTP mutation, so refresh lightly while the permission-filtered Settings
    // inventory is open. Stop after an error instead of hammering a backend.
    refetchInterval: (query) =>
      query.state.status === "success" &&
      query.state.data?.group_discovery_supported === true
        ? 5_000
        : false,
  });

export const dingtalkAgentGroupsOptions = (wsId: string, agentId: string) =>
  queryOptions({
    queryKey: dingtalkKeys.agentGroups(wsId, agentId),
    queryFn: () => api.listAgentDingTalkGroups(agentId),
    enabled: !!wsId && !!agentId,
    // Agent detail uses a separate cache and a permission-scoped endpoint; it
    // must never borrow the admin workspace inventory and filter it client-side.
    refetchInterval: (query) =>
      query.state.status === "success" &&
      query.state.data?.group_discovery_supported === true
        ? 5_000
        : false,
  });

export const dingtalkInactiveGroupsOptions = (wsId: string, installationId: string) =>
  infiniteQueryOptions({
    queryKey: dingtalkKeys.inactiveGroups(wsId, installationId),
    initialPageParam: 0,
    queryFn: ({ pageParam }) =>
      api.listDingTalkGroups(wsId, {
        activity: "inactive",
        installationId,
        offset: pageParam,
      }),
    getNextPageParam: (page) => page.next_offset,
    enabled: !!wsId && !!installationId,
  });

export const dingtalkAgentInactiveGroupsOptions = (
  wsId: string,
  agentId: string,
  installationId: string,
) =>
  infiniteQueryOptions({
    queryKey: dingtalkKeys.agentInactiveGroups(wsId, agentId, installationId),
    initialPageParam: 0,
    queryFn: ({ pageParam }) =>
      api.listAgentDingTalkGroups(agentId, {
        activity: "inactive",
        installationId,
        offset: pageParam,
      }),
    getNextPageParam: (page) => page.next_offset,
    enabled: !!wsId && !!agentId && !!installationId,
  });
