/**
 * @vitest-environment jsdom
 */
import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceChannel, WorkspaceChannelMessage } from "../types/channel";
import { useWS } from "../realtime/provider";
import { channelKeys } from "./keys";
import {
  applyWorkspaceChannelCreatedEvent,
  applyWorkspaceChannelMessageEvent,
  useChannelRealtime,
} from "./realtime";

type EventHandler = (payload: unknown, actorId?: string, actorType?: string) => void;

vi.mock("../realtime/provider", () => ({
  useWS: vi.fn(),
}));

const channel: WorkspaceChannel = {
  id: "channel-1",
  workspace_id: "ws-1",
  name: "Product planning",
  slug: "product-planning",
  description: "",
  created_by: "member-1",
  archived_at: null,
  created_at: "2026-09-02T00:00:00Z",
  updated_at: "2026-09-02T00:00:00Z",
};

const message: WorkspaceChannelMessage = {
  id: "message-1",
  workspace_id: "ws-1",
  channel_id: "channel-1",
  author_type: "member",
  author_id: "member-1",
  content: "Ship the first pass.",
  parent_id: null,
  quoted_message_id: null,
  created_at: "2026-09-02T00:01:00Z",
  updated_at: "2026-09-02T00:01:00Z",
};

function wrapper(queryClient: QueryClient) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

describe("channel realtime cache", () => {
  let queryClient: QueryClient;
  let subscribers: Map<string, EventHandler>;
  let reconnectCallbacks: Set<() => void>;
  let subscribe: ReturnType<typeof vi.fn>;
  let onReconnect: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    subscribers = new Map();
    reconnectCallbacks = new Set();
    subscribe = vi.fn((event: string, handler: EventHandler) => {
      subscribers.set(event, handler);
      return () => subscribers.delete(event);
    });
    onReconnect = vi.fn((callback: () => void) => {
      reconnectCallbacks.add(callback);
      return () => reconnectCallbacks.delete(callback);
    });
    vi.mocked(useWS).mockReturnValue({
      subscribe: subscribe as ReturnType<typeof useWS>["subscribe"],
      onReconnect: onReconnect as ReturnType<typeof useWS>["onReconnect"],
    });
  });

  afterEach(() => {
    queryClient.clear();
    vi.clearAllMocks();
  });

  it("writes a created channel and invalidates the list", () => {
    const key = channelKeys.list("ws-1");
    queryClient.setQueryData(key, { channels: [] });

    applyWorkspaceChannelCreatedEvent(queryClient, "ws-1", { channel });

    expect(queryClient.getQueryData(key)).toEqual({ channels: [channel] });
    expect(queryClient.getQueryState(key)?.isInvalidated).toBe(true);
  });

  it("updates an opened transcript and de-duplicates a repeated message event", () => {
    const key = channelKeys.messages("ws-1", "channel-1");
    queryClient.setQueryData(key, {
      pages: [
        {
          messages: [{ ...message, id: "optimistic-message-1", optimistic: true }],
          limit: 50,
          has_more: false,
          next_cursor: null,
        },
        {
          messages: [],
          limit: 50,
          has_more: false,
          next_cursor: null,
        },
      ],
      pageParams: [null, { created_at: message.created_at, id: message.id }],
    });

    applyWorkspaceChannelMessageEvent(queryClient, "ws-1", { message });
    applyWorkspaceChannelMessageEvent(queryClient, "ws-1", { message });

    expect(queryClient.getQueryData(key)).toEqual({
      pages: [
        { messages: [message], limit: 50, has_more: false, next_cursor: null },
        { messages: [], limit: 50, has_more: false, next_cursor: null },
      ],
      pageParams: [null, { created_at: message.created_at, id: message.id }],
    });
    expect(queryClient.getQueryState(key)?.isInvalidated).toBe(true);
  });

  it("subscribes to both channel events and invalidates all channel queries on reconnect", () => {
    const listKey = channelKeys.list("ws-1");
    const messageKey = channelKeys.messages("ws-1", "channel-1");
    queryClient.setQueryData(listKey, { channels: [channel] });
    queryClient.setQueryData(messageKey, { messages: [message] });

    const { unmount } = renderHook(() => useChannelRealtime("ws-1"), {
      wrapper: wrapper(queryClient),
    });

    expect(subscribe).toHaveBeenCalledTimes(2);
    expect(subscribers.has("channel:created")).toBe(true);
    expect(subscribers.has("channel:message")).toBe(true);
    expect(onReconnect).toHaveBeenCalledTimes(1);

    act(() => {
      for (const callback of reconnectCallbacks) callback();
    });

    expect(queryClient.getQueryState(listKey)?.isInvalidated).toBe(true);
    expect(queryClient.getQueryState(messageKey)?.isInvalidated).toBe(true);

    unmount();
    expect(subscribers.size).toBe(0);
    expect(reconnectCallbacks.size).toBe(0);
  });

  it("ignores a valid event from another workspace and refetches malformed payloads", () => {
    const listKey = channelKeys.list("ws-1");
    queryClient.setQueryData(listKey, { channels: [] });

    applyWorkspaceChannelCreatedEvent(queryClient, "ws-1", {
      ...channel,
      workspace_id: "ws-2",
    });
    expect(queryClient.getQueryData(listKey)).toEqual({ channels: [] });
    expect(queryClient.getQueryState(listKey)?.isInvalidated).toBe(false);

    applyWorkspaceChannelMessageEvent(queryClient, "ws-1", { channel_id: "channel-1" });
    expect(queryClient.getQueryState(listKey)?.isInvalidated).toBe(true);
  });
});
