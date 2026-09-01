import { queryOptions } from "@tanstack/react-query";
import { api } from "../api";

export const weixinKeys = {
  all: (wsId: string) => ["weixin", wsId] as const,
  installations: (wsId: string) => [...weixinKeys.all(wsId), "installations"] as const,
};

export const weixinInstallationsOptions = (wsId: string) =>
  queryOptions({
    queryKey: weixinKeys.installations(wsId),
    queryFn: () => api.listWeixinInstallations(wsId),
    enabled: !!wsId,
    refetchInterval: (query) => (query.state.status === "error" ? false : 5_000),
  });
