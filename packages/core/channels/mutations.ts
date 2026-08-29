import { useMutation, useQueryClient, type InfiniteData } from "@tanstack/react-query";
import { api } from "../api";
import { useWorkspaceId } from "../hooks";
import type {
  Channel,
  ChannelMessagesPage,
  CreateChannelRequest,
  SendChannelMessageRequest,
} from "../types";
import { channelKeys } from "./queries";

export function useCreateChannel() {
  const queryClient = useQueryClient();
  const wsId = useWorkspaceId();

  return useMutation({
    mutationFn: (request: CreateChannelRequest) => api.createChannel(request),
    onSuccess: (channel) => {
      queryClient.setQueryData<Channel[]>(channelKeys.list(wsId), (old) => {
        if (old?.some((item) => item.id === channel.id)) return old;
        return [...(old ?? []), channel];
      });
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: channelKeys.list(wsId) });
    },
  });
}

export function useSendChannelMessage() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ channelId, ...request }: SendChannelMessageRequest & { channelId: string }) =>
      api.sendChannelMessage(channelId, request),
    onSuccess: (message, variables) => {
      queryClient.setQueryData<InfiniteData<ChannelMessagesPage>>(
        channelKeys.messages(variables.channelId),
        (old) => {
          if (!old) return old;
          if (old.pages.some((page) => page.messages.some((item) => item.id === message.id))) {
            return old;
          }
          const [firstPage, ...olderPages] = old.pages;
          if (!firstPage) return old;
          return {
            ...old,
            pages: [
              { ...firstPage, messages: [...firstPage.messages, message] },
              ...olderPages,
            ],
          };
        },
      );
    },
    onSettled: (_data, _error, variables) => {
      queryClient.invalidateQueries({ queryKey: channelKeys.messages(variables.channelId) });
    },
  });
}
