import { describe, expect, it, vi } from "vitest";
import { QueryClient } from "@tanstack/react-query";
import type {
  WorkspaceChannel,
  WorkspaceChannelMessage,
  WorkspaceChannelMessageCacheEntry,
} from "@/data/channel-types";
import { channelKeys } from "@/data/queries/channels";
import type { ChannelMessagesCache } from "./channel-ws-updaters";
import {
  createOptimisticChannelMessage,
  upsertChannelMessageToCache,
  upsertChannelToCache,
} from "./channel-ws-updaters";

vi.mock("@/data/api", () => ({ api: {} }));

const WS_ID = "workspace-1";
const CHANNEL_ID = "channel-1";

function message(
  over: Partial<WorkspaceChannelMessage> = {},
): WorkspaceChannelMessage {
  return {
    id: "message-1",
    workspace_id: WS_ID,
    channel_id: CHANNEL_ID,
    author_type: "member",
    author_id: "user-1",
    content: "Hello",
    parent_id: null,
    quoted_message_id: null,
    created_at: "2026-09-02T00:00:00Z",
    updated_at: "2026-09-02T00:00:00Z",
    ...over,
  };
}

function seedMessages(
  qc: QueryClient,
  messages: WorkspaceChannelMessageCacheEntry[],
) {
  qc.setQueryData<ChannelMessagesCache>(
    channelKeys.messages(WS_ID, CHANNEL_ID),
    {
      pages: [{ messages, has_more: false, next_cursor: null }],
      pageParams: [null],
    },
  );
}

function cachedMessages(qc: QueryClient) {
  return qc.getQueryData<ChannelMessagesCache>(
    channelKeys.messages(WS_ID, CHANNEL_ID),
  )?.pages.flatMap((page) => page.messages) ?? [];
}

describe("workspace channel cache updaters", () => {
  it("upserts a channel without duplicating an echoed create event", () => {
    const qc = new QueryClient();
    const channel: WorkspaceChannel = {
      id: CHANNEL_ID,
      workspace_id: WS_ID,
      name: "Team updates",
      slug: "team-updates",
      description: "",
      created_by: "user-1",
      archived_at: null,
      created_at: "2026-09-02T00:00:00Z",
      updated_at: "2026-09-02T00:00:00Z",
    };
    qc.setQueryData<WorkspaceChannel[]>(channelKeys.list(WS_ID), []);

    upsertChannelToCache(qc, WS_ID, channel);
    upsertChannelToCache(qc, WS_ID, channel);

    expect(qc.getQueryData<WorkspaceChannel[]>(channelKeys.list(WS_ID))).toEqual([
      channel,
    ]);
  });

  it("replaces one optimistic send with the authoritative echo", () => {
    const qc = new QueryClient();
    const optimistic = createOptimisticChannelMessage(
      WS_ID,
      CHANNEL_ID,
      "user-1",
      "Hello",
    );
    seedMessages(qc, [optimistic]);

    upsertChannelMessageToCache(
      qc,
      WS_ID,
      CHANNEL_ID,
      message({ id: "message-server", created_at: "2026-09-02T00:00:01Z" }),
    );

    expect(cachedMessages(qc)).toHaveLength(1);
    expect(cachedMessages(qc)[0].id).toBe("message-server");
    expect(cachedMessages(qc)[0]).not.toHaveProperty("optimistic");
  });

  it("keeps a second identical optimistic send when the first echo arrives", () => {
    const qc = new QueryClient();
    const first = {
      ...createOptimisticChannelMessage(WS_ID, CHANNEL_ID, "user-1", "Hello"),
      created_at: "2026-09-02T00:00:00Z",
    };
    const second = {
      ...createOptimisticChannelMessage(WS_ID, CHANNEL_ID, "user-1", "Hello"),
      created_at: "2026-09-02T00:00:02Z",
    };
    seedMessages(qc, [first, second]);

    upsertChannelMessageToCache(
      qc,
      WS_ID,
      CHANNEL_ID,
      message({ id: "message-first", created_at: "2026-09-02T00:00:01Z" }),
    );

    expect(cachedMessages(qc).map((entry) => entry.id)).toEqual([
      "message-first",
      second.id,
    ]);
  });

  it("does not write a foreign workspace message into the active cache", () => {
    const qc = new QueryClient();
    seedMessages(qc, [message()]);

    upsertChannelMessageToCache(
      qc,
      WS_ID,
      CHANNEL_ID,
      message({ id: "foreign", workspace_id: "workspace-2" }),
    );

    expect(cachedMessages(qc).map((entry) => entry.id)).toEqual(["message-1"]);
  });
});
