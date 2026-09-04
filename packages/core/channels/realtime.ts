"use client";

import { useCallback, useEffect } from "react";
import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import type { WSEventType } from "../types/events";
import {
  CHANNEL_CREATED_EVENT,
  CHANNEL_MESSAGE_EVENT,
  parseWorkspaceChannelCreatedEvent,
  parseWorkspaceChannelMessageEvent,
} from "../types/channel";
import { useWS } from "../realtime/provider";
import { channelKeys } from "./keys";
import {
  upsertWorkspaceChannelMessageToCache,
  upsertWorkspaceChannelToCache,
} from "./cache";

/**
 * The shared event union has not yet registered channel events, while the
 * current Go handler does publish them. Keep the cast at this adapter
 * boundary so the rest of the channel code remains typed and the eventual
 * shared-registry change has one obvious replacement point.
 */
function asRegisteredEvent(event: typeof CHANNEL_CREATED_EVENT | typeof CHANNEL_MESSAGE_EVENT): WSEventType {
  return event as unknown as WSEventType;
}

export function invalidateWorkspaceChannelQueries(
  queryClient: QueryClient,
  workspaceId: string,
): void {
  if (!workspaceId) return;
  void queryClient.invalidateQueries({
    queryKey: channelKeys.all(workspaceId),
  });
}

export function applyWorkspaceChannelCreatedEvent(
  queryClient: QueryClient,
  workspaceId: string,
  payload: unknown,
): void {
  const channel = parseWorkspaceChannelCreatedEvent(payload);
  if (!channel) {
    void queryClient.invalidateQueries({
      queryKey: channelKeys.list(workspaceId),
    });
    return;
  }
  if (channel.workspace_id !== workspaceId) return;

  // Do not fabricate a partial list when this page has never been opened.
  // The next list query owns the initial snapshot; an existing list gets the
  // event immediately and then revalidates against the server.
  upsertWorkspaceChannelToCache(queryClient, workspaceId, channel);
  void queryClient.invalidateQueries({
    queryKey: channelKeys.list(workspaceId),
  });
}

export function applyWorkspaceChannelMessageEvent(
  queryClient: QueryClient,
  workspaceId: string,
  payload: unknown,
): void {
  const message = parseWorkspaceChannelMessageEvent(payload);
  if (!message) {
    // The malformed event has no safe channel id. Revalidate the whole channel
    // family instead of writing an unscoped row into an arbitrary transcript.
    invalidateWorkspaceChannelQueries(queryClient, workspaceId);
    return;
  }
  if (message.workspace_id !== workspaceId) return;

  // An event for an unopened channel must not turn one message into a fake
  // complete history. An active messages query is updated in place; the
  // invalidation below makes the API the authority for the final snapshot.
  upsertWorkspaceChannelMessageToCache(
    queryClient,
    workspaceId,
    message.channel_id,
    message,
  );
  void queryClient.invalidateQueries({
    queryKey: channelKeys.messages(workspaceId, message.channel_id),
  });
}

/** Subscribe the mounted channel surface and recover its caches after WS reconnect. */
export function useChannelRealtime(workspaceId: string): void {
  const queryClient = useQueryClient();
  const { subscribe, onReconnect } = useWS();

  const onChannelCreated = useCallback(
    (payload: unknown) => {
      applyWorkspaceChannelCreatedEvent(queryClient, workspaceId, payload);
    },
    [queryClient, workspaceId],
  );
  const onChannelMessage = useCallback(
    (payload: unknown) => {
      applyWorkspaceChannelMessageEvent(queryClient, workspaceId, payload);
    },
    [queryClient, workspaceId],
  );
  const onReconnectCallback = useCallback(() => {
    invalidateWorkspaceChannelQueries(queryClient, workspaceId);
  }, [queryClient, workspaceId]);

  useEffect(() => {
    const unsubscribeCreated = subscribe(
      asRegisteredEvent(CHANNEL_CREATED_EVENT),
      onChannelCreated,
    );
    const unsubscribeMessage = subscribe(
      asRegisteredEvent(CHANNEL_MESSAGE_EVENT),
      onChannelMessage,
    );
    return () => {
      unsubscribeCreated();
      unsubscribeMessage();
    };
  }, [onChannelCreated, onChannelMessage, subscribe]);

  useEffect(() => onReconnect(onReconnectCallback), [onReconnect, onReconnectCallback]);
}
