import type { InfiniteData, QueryClient } from "@tanstack/react-query";
import type { WorkspaceChannel, WorkspaceChannelMessage, WorkspaceChannelMessageCacheEntry } from "@/data/channel-types";
import { channelKeys } from "@/data/queries/channels";

export type ChannelMessagesPage = Omit<
  import("@/data/channel-types").ListWorkspaceChannelMessagesResponse,
  "messages"
> & {
  messages: WorkspaceChannelMessageCacheEntry[];
};

export type ChannelMessagesCache = InfiniteData<
  ChannelMessagesPage,
  import("@/data/channel-types").WorkspaceChannelMessageCursor | null
>;

function timestampOf(value: string): number {
  const timestamp = Date.parse(value);
  return Number.isFinite(timestamp) ? timestamp : 0;
}

function compareMessages(
  left: WorkspaceChannelMessageCacheEntry,
  right: WorkspaceChannelMessageCacheEntry,
): number {
  const byTime = timestampOf(left.created_at) - timestampOf(right.created_at);
  return byTime || left.id.localeCompare(right.id);
}

function sameMessageRequest(
  left: WorkspaceChannelMessageCacheEntry,
  right: WorkspaceChannelMessage,
): boolean {
  return (
    left.optimistic === true &&
    left.workspace_id === right.workspace_id &&
    left.channel_id === right.channel_id &&
    left.author_type === right.author_type &&
    left.author_id === right.author_id &&
    left.content === right.content &&
    left.parent_id === right.parent_id &&
    left.quoted_message_id === right.quoted_message_id
  );
}

function closestOptimisticIndex(
  entries: WorkspaceChannelMessageCacheEntry[],
  message: WorkspaceChannelMessage,
): number {
  let match = -1;
  let distance = Number.POSITIVE_INFINITY;
  const timestamp = timestampOf(message.created_at);
  entries.forEach((entry, index) => {
    if (!sameMessageRequest(entry, message)) return;
    const nextDistance = Math.abs(timestampOf(entry.created_at) - timestamp);
    if (nextDistance < distance) {
      match = index;
      distance = nextDistance;
    }
  });
  return match;
}

export function upsertChannelToCache(
  qc: QueryClient,
  wsId: string,
  channel: WorkspaceChannel,
) {
  if (!wsId || channel.workspace_id !== wsId || !channel.id) return;
  qc.setQueryData<WorkspaceChannel[]>(channelKeys.list(wsId), (old) => {
    if (!old) return old;
    const index = old.findIndex((entry) => entry.id === channel.id);
    if (index === -1) return [...old, channel];
    const next = old.slice();
    next[index] = channel;
    return next;
  });
}

export function createOptimisticChannelMessage(
  wsId: string,
  channelId: string,
  authorId: string,
  content: string,
  parentId: string | null = null,
  quotedMessageId: string | null = null,
): WorkspaceChannelMessageCacheEntry {
  const id = `optimistic-channel-message-${Date.now()}-${Math.random()
    .toString(36)
    .slice(2)}`;
  const now = new Date().toISOString();
  return {
    id,
    workspace_id: wsId,
    channel_id: channelId,
    author_type: "member",
    author_id: authorId,
    content,
    parent_id: parentId,
    quoted_message_id: quotedMessageId,
    created_at: now,
    updated_at: now,
    optimistic: true,
  };
}

export function upsertChannelMessageToCache(
  qc: QueryClient,
  wsId: string,
  channelId: string,
  message: WorkspaceChannelMessage,
  options: { seedIfMissing?: boolean } = {},
) {
  if (
    !wsId ||
    !channelId ||
    message.workspace_id !== wsId ||
    message.channel_id !== channelId ||
    !message.id
  ) {
    return;
  }

  const key = channelKeys.messages(wsId, channelId);
  qc.setQueryData<ChannelMessagesCache>(key, (old) => {
    if (!old) {
      if (!options.seedIfMissing) return old;
      return {
        pages: [
          {
            messages: [message],
            has_more: false,
            next_cursor: null,
          },
        ],
        pageParams: [null],
      };
    }

    const pages = old.pages.map((page) => ({
      ...page,
      messages: page.messages.slice(),
    }));
    let pageIndex = -1;
    let entryIndex = -1;
    let optimisticToRemove: string | null = null;

    for (let i = 0; i < pages.length; i += 1) {
      const exact = pages[i].messages.findIndex((entry) => entry.id === message.id);
      if (exact !== -1) {
        pageIndex = i;
        entryIndex = exact;
        break;
      }
    }

    if (pageIndex === -1) {
      for (let i = 0; i < pages.length; i += 1) {
        const optimistic = closestOptimisticIndex(pages[i].messages, message);
        if (optimistic !== -1) {
          pageIndex = i;
          entryIndex = optimistic;
          optimisticToRemove = pages[i].messages[optimistic].id;
          break;
        }
      }
    }

    if (pageIndex === -1) {
      pageIndex = 0;
      pages[0].messages.push(message);
    } else {
      pages[pageIndex].messages[entryIndex] = message;
    }

    // Reconciliation is intentionally global: a reconnect can replay the
    // same authoritative row while an optimistic row sits in another page.
    // Remove duplicates by server id and remove the optimistic request that
    // the response echoes, then sort each page for stable rendering.
    let authoritativeSeen = false;
    for (const page of pages) {
      page.messages = page.messages.filter((entry) => {
        if (entry.id === message.id) {
          if (authoritativeSeen) return false;
          authoritativeSeen = true;
          return true;
        }
        if (entry.id === optimisticToRemove) return false;
        return true;
      });
      page.messages.sort(compareMessages);
    }

    return { ...old, pages };
  });
}

export function removeChannelMessageFromCache(
  qc: QueryClient,
  wsId: string,
  channelId: string,
  messageId: string,
) {
  if (!wsId || !channelId || !messageId) return;
  qc.setQueryData<ChannelMessagesCache>(
    channelKeys.messages(wsId, channelId),
    (old) => {
      if (!old) return old;
      return {
        ...old,
        pages: old.pages.map((page) => ({
          ...page,
          messages: page.messages.filter((message) => message.id !== messageId),
        })),
      };
    },
  );
}

export function flattenChannelMessages(
  pages: ChannelMessagesPage[] | undefined,
): WorkspaceChannelMessageCacheEntry[] {
  if (!pages) return [];
  const seen = new Set<string>();
  return pages
    .flatMap((page) => page.messages)
    .filter((message) => {
      if (seen.has(message.id)) return false;
      seen.add(message.id);
      return true;
    })
    .sort(compareMessages);
}
