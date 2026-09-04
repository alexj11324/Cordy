import {
  infiniteQueryOptions,
  queryOptions,
} from "@tanstack/react-query";
import type {
  WorkspaceChannelMessageCursor,
} from "@/data/channel-types";
import { api } from "@/data/api";

export const CHANNEL_MESSAGE_PAGE_SIZE = 50;

export const channelKeys = {
  all: (wsId: string | null) => ["workspace-channels", wsId] as const,
  list: (wsId: string | null) => [...channelKeys.all(wsId), "list"] as const,
  messages: (wsId: string | null, channelId: string) =>
    [...channelKeys.all(wsId), "messages", channelId] as const,
};

export const channelListOptions = (wsId: string | null) =>
  queryOptions({
    queryKey: channelKeys.list(wsId),
    queryFn: async ({ signal }) => {
      const response = await api.listWorkspaceChannels({ signal });
      return response.channels;
    },
    enabled: !!wsId,
  });

export const channelMessagesOptions = (
  wsId: string | null,
  channelId: string,
  limit = CHANNEL_MESSAGE_PAGE_SIZE,
) =>
  infiniteQueryOptions({
    queryKey: channelKeys.messages(wsId, channelId),
    queryFn: ({ pageParam, signal }) =>
      api.listWorkspaceChannelMessages(
        channelId,
        {
          limit,
          before: pageParam,
        },
        { signal },
      ),
    initialPageParam: null as WorkspaceChannelMessageCursor | null,
    getNextPageParam: (lastPage) =>
      lastPage.has_more && lastPage.next_cursor
        ? lastPage.next_cursor
        : undefined,
    enabled: !!wsId && !!channelId,
  });
