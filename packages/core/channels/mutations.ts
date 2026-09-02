import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useWorkspaceId } from "../hooks";
import { api } from "../api";
import type {
  CreateWorkspaceChannelMessageRequest,
  CreateWorkspaceChannelRequest,
} from "../types";
import {
  parseWorkspaceChannelCreatedEvent,
  parseWorkspaceChannelMessageEvent,
} from "../types/channel";
import type { WorkspaceChannelMessagesData } from "../types/channel";
import {
  createOptimisticWorkspaceChannelMessage,
  removeWorkspaceChannelMessageFromCache,
  upsertWorkspaceChannelMessageToCache,
  upsertWorkspaceChannelToCache,
} from "./cache";
import { channelKeys } from "./queries";

export function useCreateWorkspaceChannel() {
  const queryClient = useQueryClient();
  const wsId = useWorkspaceId();

  return useMutation({
    mutationFn: async (data: CreateWorkspaceChannelRequest) => {
      const channel = parseWorkspaceChannelCreatedEvent(
        await api.createWorkspaceChannel(data),
      );
      if (!channel || channel.workspace_id !== wsId) {
        throw new Error("Invalid workspace channel response");
      }
      return channel;
    },
    onSuccess: (channel) => {
      upsertWorkspaceChannelToCache(queryClient, wsId, channel);
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: channelKeys.list(wsId) });
    },
  });
}

export type CreateWorkspaceChannelMessageVariables =
  CreateWorkspaceChannelMessageRequest & { channelId: string };

export function useCreateWorkspaceChannelMessage() {
  const queryClient = useQueryClient();
  const wsId = useWorkspaceId();

  return useMutation({
    mutationFn: async ({ channelId, ...data }: CreateWorkspaceChannelMessageVariables) => {
      const message = parseWorkspaceChannelMessageEvent(
        await api.createWorkspaceChannelMessage(channelId, data),
      );
      if (
        !message ||
        message.workspace_id !== wsId ||
        message.channel_id !== channelId
      ) {
        throw new Error("Invalid workspace channel message response");
      }
      return message;
    },
    onMutate: async ({ channelId, ...data }) => {
      const queryKey = channelKeys.messages(wsId, channelId);
      const hadCachedTranscript =
        queryClient.getQueryData<WorkspaceChannelMessagesData>(queryKey) !== undefined;
      await queryClient.cancelQueries({ queryKey });
      const optimistic = createOptimisticWorkspaceChannelMessage(
        wsId,
        channelId,
        data,
      );
      upsertWorkspaceChannelMessageToCache(
        queryClient,
        wsId,
        channelId,
        optimistic,
        { seedIfMissing: true },
      );
      return {
        channelId,
        optimisticId: optimistic.id,
        hadCachedTranscript,
      };
    },
    onSuccess: (message, { channelId }, context) => {
      // Use the mutation context first: the Go API derives the persisted actor
      // from authentication and may legitimately return a different
      // author_type/author_id than the client placeholder. Removing by the
      // placeholder id keeps HTTP success reconciliation deterministic; the
      // cache helper still de-duplicates a WS echo by the server id.
      if (context) {
        removeWorkspaceChannelMessageFromCache(
          queryClient,
          wsId,
          channelId,
          context.optimisticId,
        );
      }
      upsertWorkspaceChannelMessageToCache(
        queryClient,
        wsId,
        channelId,
        message,
        { seedIfMissing: true },
      );
    },
    onError: (_error, { channelId }, context) => {
      if (!context) return;
      removeWorkspaceChannelMessageFromCache(
        queryClient,
        wsId,
        channelId,
        context.optimisticId,
      );
      if (!context.hadCachedTranscript) {
        const current = queryClient.getQueryData<WorkspaceChannelMessagesData>(
          channelKeys.messages(wsId, channelId),
        );
        const isEmpty = current
          ? "pages" in current
            ? current.pages.every((page) => page.messages.length === 0)
            : current.messages.length === 0
          : false;
        if (isEmpty) {
          queryClient.removeQueries({
            queryKey: channelKeys.messages(wsId, channelId),
            exact: true,
          });
        }
      }
    },
    onSettled: (_data, _error, { channelId }) => {
      void queryClient.invalidateQueries({
        queryKey: channelKeys.messages(wsId, channelId),
      });
    },
  });
}
