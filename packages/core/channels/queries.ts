import {
  infiniteQueryOptions,
  queryOptions,
  type InfiniteData,
} from "@tanstack/react-query";
import { api } from "../api";
import { channelKeys } from "./keys";
import { mergeWorkspaceChannelMessageInfiniteData } from "./cache";
import {
  normalizeWorkspaceChannelsResponse,
  normalizeWorkspaceChannelMessagesResponse,
  type ListWorkspaceChannelMessagesResponse,
  type WorkspaceChannelMessageCursor,
} from "../types/channel";

export { channelKeys } from "./keys";

export function channelListOptions(wsId: string) {
  return queryOptions({
    queryKey: channelKeys.list(wsId),
    queryFn: async () =>
      normalizeWorkspaceChannelsResponse(await api.listWorkspaceChannels()),
    enabled: Boolean(wsId),
  });
}

export function channelDetailOptions(wsId: string, channelId: string) {
  return queryOptions({
    queryKey: channelKeys.detail(wsId, channelId),
    queryFn: () => api.getWorkspaceChannel(channelId),
    enabled: Boolean(wsId && channelId),
  });
}

export const CHANNEL_MESSAGE_PAGE_SIZE = 50;

type ChannelMessageInfiniteData = InfiniteData<
  ListWorkspaceChannelMessagesResponse,
  WorkspaceChannelMessageCursor | null
>;

function isChannelMessageInfiniteData(
  value: unknown,
): value is ChannelMessageInfiniteData {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as { pages?: unknown; pageParams?: unknown };
  if (!Array.isArray(candidate.pages) || !Array.isArray(candidate.pageParams)) {
    return false;
  }
  return candidate.pages.every((page) => {
    if (typeof page !== "object" || page === null) return false;
    return Array.isArray((page as { messages?: unknown }).messages);
  });
}

function mergeChannelMessageQueryData(
  previous: unknown,
  incoming: unknown,
): unknown {
  if (!isChannelMessageInfiniteData(incoming)) return incoming;
  return mergeWorkspaceChannelMessageInfiniteData(
    isChannelMessageInfiniteData(previous) ? previous : undefined,
    incoming,
  );
}

export function channelMessagesOptions(
  wsId: string,
  channelId: string,
  limit = CHANNEL_MESSAGE_PAGE_SIZE,
) {
  return infiniteQueryOptions({
    queryKey: channelKeys.messages(wsId, channelId),
    queryFn: async ({ pageParam }) =>
      normalizeWorkspaceChannelMessagesResponse(
        await api.listWorkspaceChannelMessages(channelId, {
          before: pageParam,
          limit,
        }),
      ),
    initialPageParam: null as WorkspaceChannelMessageCursor | null,
    getNextPageParam: (lastPage) =>
      lastPage.has_more ? lastPage.next_cursor ?? undefined : undefined,
    enabled: Boolean(wsId && channelId),
    structuralSharing: mergeChannelMessageQueryData,
  });
}
