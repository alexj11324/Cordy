import { QueryClient, type InfiniteData } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setApiInstance } from "../api";
import { ApiClient } from "../api/client";
import type {
  ListWorkspaceChannelMessagesResponse,
  WorkspaceChannelMessageCursor,
} from "../types/channel";
import {
  mergeWorkspaceChannelMessageInfiniteData,
  mergeWorkspaceChannelMessageResponses,
  upsertWorkspaceChannelMessageToCache,
} from "./cache";
import { channelMessagesOptions } from "./queries";
import {
  normalizeWorkspaceChannelMessagesResponse,
  normalizeWorkspaceChannelsResponse,
} from "../types/channel";

afterEach(() => {
  vi.unstubAllGlobals();
});

const channel = {
  id: "channel-1",
  workspace_id: "workspace-1",
  name: "Product planning",
  slug: "product-planning",
  description: "Shared planning",
  created_by: "member-1",
  archived_at: null,
  created_at: "2026-09-02T00:00:00Z",
  updated_at: "2026-09-02T00:00:00Z",
};

const message = {
  id: "message-1",
  workspace_id: "workspace-1",
  channel_id: "channel-1",
  author_type: "member",
  author_id: "member-1",
  content: "Ship the first pass.",
  parent_id: null,
  quoted_message_id: null,
  created_at: "2026-09-02T00:01:00Z",
  updated_at: "2026-09-02T00:01:00Z",
};

function jsonResponse(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

describe("ApiClient workspace channel contract", () => {
  it("uses the Go list/create/channel-message endpoints and envelopes", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse({ channels: [channel] }))
      .mockResolvedValueOnce(jsonResponse(channel, 201))
      .mockResolvedValueOnce(jsonResponse(channel))
      .mockResolvedValueOnce(
        jsonResponse({
          messages: [message],
          limit: 50,
          has_more: true,
          next_cursor: {
            created_at: "2026-09-01T23:59:00Z",
            id: "message-0",
          },
        }),
      )
      .mockResolvedValueOnce(jsonResponse(message, 201));
    vi.stubGlobal("fetch", fetchMock);

    const client = new ApiClient("https://api.example.test");
    await expect(client.listWorkspaceChannels()).resolves.toEqual({ channels: [channel] });
    await expect(
      client.createWorkspaceChannel({
        name: channel.name,
        slug: channel.slug,
        description: channel.description,
      }),
    ).resolves.toEqual(channel);
    await expect(client.getWorkspaceChannel("channel/1")).resolves.toEqual(channel);
    await expect(client.listWorkspaceChannelMessages(channel.id)).resolves.toEqual({
      messages: [message],
      limit: 50,
      has_more: true,
      next_cursor: {
        created_at: "2026-09-01T23:59:00Z",
        id: "message-0",
      },
    });
    await expect(
      client.createWorkspaceChannelMessage(channel.id, {
        author_type: "member",
        author_id: "member-1",
        content: message.content,
      }),
    ).resolves.toEqual(message);

    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
      "https://api.example.test/api/workspace-channels",
      "https://api.example.test/api/workspace-channels",
      "https://api.example.test/api/workspace-channels/channel%2F1",
      "https://api.example.test/api/workspace-channels/channel-1/messages",
      "https://api.example.test/api/workspace-channels/channel-1/messages",
    ]);
    expect(JSON.parse(String(fetchMock.mock.calls[1]?.[1]?.body))).toEqual({
      name: channel.name,
      slug: channel.slug,
      description: channel.description,
    });
    expect(JSON.parse(String(fetchMock.mock.calls[4]?.[1]?.body))).toEqual({
      author_type: "member",
      author_id: "member-1",
      content: message.content,
    });
  });

  it("falls back to empty envelopes for malformed channel responses", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValueOnce(jsonResponse({ channels: "not-an-array" })),
    );

    await expect(
      new ApiClient("https://api.example.test").listWorkspaceChannels(),
    ).resolves.toEqual({ channels: [] });
  });

  it("serializes the Go cursor page parameters through the shared client", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        jsonResponse({ messages: [], limit: 25, has_more: false, next_cursor: null }),
      );
    vi.stubGlobal("fetch", fetchMock);

    await new ApiClient("https://api.example.test").listWorkspaceChannelMessages(
      "channel/1",
      {
        limit: 25,
        before: {
          created_at: "2026-09-01T23:59:00Z",
          id: "message-0",
        },
      },
    );

    expect(fetchMock.mock.calls[0]?.[0]).toBe(
      "https://api.example.test/api/workspace-channels/channel%2F1/messages?limit=25&before_created_at=2026-09-01T23%3A59%3A00Z&before_id=message-0",
    );
  });

  it("drops malformed channel rows before they enter list state", () => {
    expect(
      normalizeWorkspaceChannelsResponse({
        channels: [channel, { ...channel, id: "" }, { ...channel, archived_at: 42 }],
      }),
    ).toEqual({ channels: [channel] });
  });

  it("keeps a cursor-shaped response while reconciling an optimistic row", () => {
    const optimistic = {
      ...message,
      id: "optimistic-channel-message-1",
      optimistic: true,
    };
    const nextCursor = {
      created_at: "2026-09-01T23:59:00Z",
      id: "message-0",
    };

    const merged = mergeWorkspaceChannelMessageResponses(
      { messages: [optimistic] },
      { messages: [message], limit: 64, has_more: true, next_cursor: nextCursor },
    );

    expect(merged.messages).toEqual([message]);
    expect(merged.limit).toBe(64);
    expect(merged.has_more).toBe(true);
    expect(merged.next_cursor).toEqual(nextCursor);
  });

  it("preserves additive author values and stops on malformed cursor metadata", () => {
    const normalized = normalizeWorkspaceChannelMessagesResponse({
      messages: [{ ...message, author_type: "future_actor" }],
      limit: 64,
      has_more: true,
      next_cursor: {
        created_at: "2026-09-01T23:59:00Z",
        id: "message-0",
      },
    });

    expect(normalized.messages[0]?.author_type).toBe("future_actor");
    expect(normalized.has_more).toBe(true);
    expect(normalized.next_cursor).toEqual({
      created_at: "2026-09-01T23:59:00Z",
      id: "message-0",
    });

    const unsafe = normalizeWorkspaceChannelMessagesResponse({
      messages: [message],
      limit: 64,
      has_more: true,
      next_cursor: { created_at: "2026-09-01T23:59:00Z", id: 0 },
    });

    expect(unsafe.has_more).toBe(false);
    expect(unsafe.next_cursor).toBeNull();

    expect(
      normalizeWorkspaceChannelMessagesResponse({
        messages: [{ ...message, content: "  " }],
      }).messages,
    ).toEqual([]);
  });

  it("uses the server cursor as the next infinite page parameter", () => {
    const options = channelMessagesOptions("workspace-1", "channel-1", 25);
    const nextCursor: WorkspaceChannelMessageCursor = {
      created_at: "2026-09-01T23:59:00Z",
      id: "message-0",
    };
    const page: ListWorkspaceChannelMessagesResponse = {
      messages: [message],
      limit: 25,
      has_more: true,
      next_cursor: nextCursor,
    };

    expect(options.initialPageParam).toBeNull();
    expect(options.getNextPageParam(page, [page], null, [null])).toEqual(nextCursor);
    expect(
      options.getNextPageParam(
        { ...page, has_more: false, next_cursor: null },
        [page],
        nextCursor,
        [null, nextCursor],
      ),
    ).toBeUndefined();
  });

  it("passes each infinite page cursor to the shared API method", async () => {
    const nextCursor: WorkspaceChannelMessageCursor = {
      created_at: "2026-09-01T23:59:00Z",
      id: "message-0",
    };
    const listMessages = vi
      .fn()
      .mockResolvedValueOnce({
        messages: [message],
        limit: 25,
        has_more: true,
        next_cursor: nextCursor,
      })
      .mockResolvedValueOnce({
        messages: [],
        limit: 25,
        has_more: false,
        next_cursor: null,
      });
    setApiInstance({ listWorkspaceChannelMessages: listMessages } as unknown as ApiClient);

    const options = channelMessagesOptions("workspace-1", "channel-1", 25);
    if (!options.queryFn) throw new Error("channel message query function is missing");
    const queryClient = new QueryClient();

    await options.queryFn({
      client: queryClient,
      queryKey: options.queryKey,
      pageParam: null,
      direction: "forward",
      signal: new AbortController().signal,
      meta: undefined,
    });
    await options.queryFn({
      client: queryClient,
      queryKey: options.queryKey,
      pageParam: nextCursor,
      direction: "forward",
      signal: new AbortController().signal,
      meta: undefined,
    });

    expect(listMessages).toHaveBeenNthCalledWith(1, "channel-1", {
      before: null,
      limit: 25,
    });
    expect(listMessages).toHaveBeenNthCalledWith(2, "channel-1", {
      before: nextCursor,
      limit: 25,
    });
  });

  it("keeps optimistic rows while merging every loaded infinite page", () => {
    const optimistic = {
      ...message,
      id: "optimistic-channel-message-1",
      optimistic: true,
    };
    const olderMessage = { ...message, id: "message-0", content: "Earlier" };
    const existing: InfiniteData<
      ListWorkspaceChannelMessagesResponse,
      WorkspaceChannelMessageCursor | null
    > = {
      pages: [
        { messages: [optimistic], limit: 50, has_more: true, next_cursor: null },
        { messages: [olderMessage], limit: 50, has_more: false, next_cursor: null },
      ],
      pageParams: [null, { created_at: olderMessage.created_at, id: olderMessage.id }],
    };
    const incoming: InfiniteData<
      ListWorkspaceChannelMessagesResponse,
      WorkspaceChannelMessageCursor | null
    > = {
      pages: [
        { messages: [message], limit: 50, has_more: true, next_cursor: null },
        { messages: [olderMessage], limit: 50, has_more: false, next_cursor: null },
      ],
      pageParams: existing.pageParams,
    };

    const merged = mergeWorkspaceChannelMessageInfiniteData(existing, incoming);

    expect(merged.pages[0]?.messages).toEqual([message]);
    expect(merged.pages[1]?.messages).toEqual([olderMessage]);
    expect(merged.pageParams).toEqual(existing.pageParams);
  });

  it("removes an optimistic copy when the server row lands in another page", () => {
    const optimistic = {
      ...message,
      id: "optimistic-channel-message-1",
      optimistic: true,
    };
    const existing: InfiniteData<
      ListWorkspaceChannelMessagesResponse,
      WorkspaceChannelMessageCursor | null
    > = {
      pages: [
        { messages: [optimistic], limit: 50, has_more: true, next_cursor: null },
        { messages: [], limit: 50, has_more: false, next_cursor: null },
      ],
      pageParams: [null, { created_at: "2026-09-01T23:59:00Z", id: "message-0" }],
    };
    const incoming: InfiniteData<
      ListWorkspaceChannelMessagesResponse,
      WorkspaceChannelMessageCursor | null
    > = {
      pages: [
        { messages: [], limit: 50, has_more: true, next_cursor: null },
        { messages: [message], limit: 50, has_more: false, next_cursor: null },
      ],
      pageParams: existing.pageParams,
    };

    const merged = mergeWorkspaceChannelMessageInfiniteData(existing, incoming);

    expect(merged.pages[0]?.messages).toEqual([]);
    expect(merged.pages[1]?.messages).toEqual([message]);
  });

  it("updates a message in its loaded page instead of duplicating it on page one", () => {
    const queryClient = new QueryClient();
    const olderMessage = { ...message, id: "message-0", content: "Earlier" };
    const updatedMessage = { ...message, content: "Updated" };
    queryClient.setQueryData(channelMessagesOptions("workspace-1", "channel-1").queryKey, {
      pages: [
        { messages: [message], limit: 50, has_more: true, next_cursor: null },
        { messages: [olderMessage], limit: 50, has_more: false, next_cursor: null },
      ],
      pageParams: [null, { created_at: olderMessage.created_at, id: olderMessage.id }],
    });

    upsertWorkspaceChannelMessageToCache(
      queryClient,
      "workspace-1",
      "channel-1",
      updatedMessage,
    );

    const cached = queryClient.getQueryData<{
      pages: Array<{ messages: Array<typeof message> }>;
    }>(channelMessagesOptions("workspace-1", "channel-1").queryKey);
    expect(cached?.pages[0]?.messages).toEqual([updatedMessage]);
    expect(cached?.pages[1]?.messages).toEqual([olderMessage]);
  });
});
