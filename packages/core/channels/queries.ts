import { queryOptions } from "@tanstack/react-query";
import { api } from "../api";

export const channelKeys = {
  all: (wsId: string) => ["channels", wsId] as const,
  list: (wsId: string) => [...channelKeys.all(wsId), "list"] as const,
  detail: (wsId: string, channelId: string) =>
    [...channelKeys.all(wsId), "detail", channelId] as const,
  messagesAll: () => ["channels", "messages"] as const,
  messages: (channelId: string) => [...channelKeys.messagesAll(), channelId] as const,
};

export function channelListOptions(wsId: string) {
  return queryOptions({
    queryKey: channelKeys.list(wsId),
    queryFn: () => api.listChannels(),
    staleTime: Infinity,
  });
}

export function channelDetailOptions(wsId: string, channelId: string) {
  return queryOptions({
    queryKey: channelKeys.detail(wsId, channelId),
    queryFn: () => api.getChannel(channelId),
    enabled: !!channelId,
    staleTime: Infinity,
  });
}

export function channelMessagesOptions(channelId: string) {
  return queryOptions({
    queryKey: channelKeys.messages(channelId),
    queryFn: () => api.listChannelMessages(channelId),
    enabled: !!channelId,
    staleTime: 0,
  });
}
