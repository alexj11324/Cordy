import { queryOptions } from "@tanstack/react-query";
import { api } from "@/data/api";

export const teamListOptions = (wsId: string | null) =>
  queryOptions({
    queryKey: ["teams", wsId] as const,
    queryFn: ({ signal }) => api.listTeams({ signal }),
    enabled: !!wsId,
  });
