import { queryOptions } from "@tanstack/react-query";
import { api } from "../api";

/** Query keys for the workspace's Weixin iLink installations. */
export const weixinKeys = {
  all: (wsId: string) => ["weixin", wsId] as const,
  installations: (wsId: string) => [...weixinKeys.all(wsId), "installations"] as const,
};

export const weixinInstallationsOptions = (wsId: string) =>
  queryOptions({
    queryKey: weixinKeys.installations(wsId),
    queryFn: () => api.listWeixinInstallations(wsId),
    enabled: !!wsId,
    refetchInterval: (query) => query.state.status === "success" ? 5_000 : false,
  });
