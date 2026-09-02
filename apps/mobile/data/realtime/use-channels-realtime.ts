import { useQueryClient } from "@tanstack/react-query";
import type { WSEventType } from "@patchbay/core/types";
import { channelKeys } from "@/data/queries/channels";
import {
  parseWorkspaceChannelCreatedEvent,
  parseWorkspaceChannelMessageEvent,
} from "@/data/channel-types";
import {
  upsertChannelMessageToCache,
  upsertChannelToCache,
} from "./channel-ws-updaters";
import { useWSSubscriptions } from "@/lib/use-ws-subscriptions";

// The Go snapshot and the incoming shared channel package publish these
// names, but the committed mobile WSEventType registry predates channels.
// Keep the cast at this adapter boundary instead of widening the shared
// registry with a duplicate local payload contract.
const CHANNEL_CREATED_EVENT = "channel:created" as unknown as WSEventType;
const CHANNEL_MESSAGE_EVENT = "channel:message" as unknown as WSEventType;

export function useChannelsRealtime() {
  const qc = useQueryClient();

  useWSSubscriptions(
    (ws, wsId) => {
      const invalidateAllChannels = () =>
        qc.invalidateQueries({ queryKey: channelKeys.all(wsId) });

      return [
        ws.on(CHANNEL_CREATED_EVENT, (payload) => {
          const channel = parseWorkspaceChannelCreatedEvent(payload);
          if (!channel || channel.workspace_id !== wsId) return;
          const listKey = channelKeys.list(wsId);
          if (qc.getQueryData(listKey)) {
            upsertChannelToCache(qc, wsId, channel);
          } else {
            void qc.invalidateQueries({ queryKey: listKey });
          }
        }),
        ws.on(CHANNEL_MESSAGE_EVENT, (payload) => {
          const message = parseWorkspaceChannelMessageEvent(payload);
          if (
            !message ||
            message.workspace_id !== wsId ||
            !message.channel_id
          ) {
            return;
          }
          upsertChannelMessageToCache(
            qc,
            wsId,
            message.channel_id,
            message,
          );
        }),
        ws.onReconnect(invalidateAllChannels),
      ];
    },
    [qc],
  );
}
