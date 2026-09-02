import { queryOptions } from "@tanstack/react-query";
import { api } from "@/data/api";

export const wecomKeys = {
  all: (wsId: string | null) => ["wecom", wsId] as const,
  installations: (wsId: string | null) =>
    [...wecomKeys.all(wsId), "installations"] as const,
};

export const wecomInstallationsOptions = (wsId: string | null) =>
  queryOptions({
    queryKey: wecomKeys.installations(wsId),
    queryFn: ({ signal }) => api.listWecomInstallations(wsId!, { signal }),
    enabled: !!wsId,
  });
