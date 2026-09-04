import { useMutation, useQueryClient } from "@tanstack/react-query";
import type {
  CreateWorkspaceChannelMessageRequest,
  CreateWorkspaceChannelRequest,
  WorkspaceChannel,
} from "@/data/channel-types";
import { api } from "@/data/api";
import { useAuthStore } from "@/data/auth-store";
import { useWorkspaceStore } from "@/data/workspace-store";
import { channelKeys } from "@/data/queries/channels";
import {
  createOptimisticChannelMessage,
  removeChannelMessageFromCache,
  upsertChannelMessageToCache,
  upsertChannelToCache,
} from "@/data/realtime/channel-ws-updaters";

export function useCreateWorkspaceChannel() {
  const qc = useQueryClient();
  const wsId = useWorkspaceStore((state) => state.currentWorkspaceId);

  return useMutation({
    mutationKey: ["createWorkspaceChannel", wsId] as const,
    mutationFn: async (body: CreateWorkspaceChannelRequest) => {
      if (!wsId) throw new Error("Workspace is not selected");
      const channel = await api.createWorkspaceChannel(body);
      if (channel.workspace_id !== wsId) {
        throw new Error("Channel belongs to a different workspace");
      }
      return channel;
    },
    onSuccess: (channel: WorkspaceChannel) => {
      upsertChannelToCache(qc, wsId ?? "", channel);
      // The list cache may not have been mounted when a deep link created a
      // channel. Seed it only with an authoritative response, never from a
      // realtime event for an unseen workspace.
      qc.setQueryData<WorkspaceChannel[]>(channelKeys.list(wsId), (old) =>
        old
          ? old.some((entry) => entry.id === channel.id)
            ? old.map((entry) => (entry.id === channel.id ? channel : entry))
            : [...old, channel]
          : [channel],
      );
    },
    onSettled: () => {
      void qc.invalidateQueries({ queryKey: channelKeys.list(wsId) });
    },
  });
}

export type CreateWorkspaceChannelMessageVariables =
  CreateWorkspaceChannelMessageRequest & {
    channelId: string;
  };

export function useCreateWorkspaceChannelMessage() {
  const qc = useQueryClient();
  const wsId = useWorkspaceStore((state) => state.currentWorkspaceId);
  const authorId = useAuthStore((state) => state.user?.id ?? null);

  return useMutation({
    mutationKey: ["createWorkspaceChannelMessage", wsId] as const,
    mutationFn: async ({ channelId, ...body }: CreateWorkspaceChannelMessageVariables) => {
      if (!wsId || !authorId) throw new Error("Sign in to send a message");
      const content = body.content.trim();
      if (!content) throw new Error("Message cannot be empty");
      const message = await api.createWorkspaceChannelMessage(channelId, {
        ...body,
        content,
      });
      if (message.workspace_id !== wsId || message.channel_id !== channelId) {
        throw new Error("Message belongs to a different workspace or channel");
      }
      return message;
    },
    onMutate: async ({ channelId, ...body }) => {
      if (!wsId || !authorId || !body.content.trim()) return undefined;
      const key = channelKeys.messages(wsId, channelId);
      await qc.cancelQueries({ queryKey: key });
      const optimistic = createOptimisticChannelMessage(
        wsId,
        channelId,
        authorId,
        body.content.trim(),
        body.parent_id ?? null,
        body.quoted_message_id ?? null,
      );
      upsertChannelMessageToCache(qc, wsId, channelId, optimistic, {
        seedIfMissing: true,
      });
      return { channelId, optimisticId: optimistic.id };
    },
    onError: (_error, variables, context) => {
      if (context && wsId) {
        removeChannelMessageFromCache(
          qc,
          wsId,
          context.channelId,
          context.optimisticId,
        );
      } else if (wsId) {
        // A synchronous validation error can happen before onMutate returns.
        // It cannot have inserted an optimistic row, so there is nothing to
        // roll back; leave the authoritative cache untouched.
        void variables;
      }
    },
    onSuccess: (message, variables, context) => {
      if (!wsId) return;
      if (context) {
        removeChannelMessageFromCache(
          qc,
          wsId,
          variables.channelId,
          context.optimisticId,
        );
      }
      upsertChannelMessageToCache(qc, wsId, variables.channelId, message, {
        seedIfMissing: true,
      });
    },
    onSettled: (_data, _error, variables) => {
      if (wsId && variables?.channelId) {
        void qc.invalidateQueries({
          queryKey: channelKeys.messages(wsId, variables.channelId),
        });
      }
    },
  });
}
