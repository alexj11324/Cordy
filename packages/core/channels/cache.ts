import type { InfiniteData, QueryClient } from "@tanstack/react-query";
import { channelKeys } from "./keys";
import type {
  ListWorkspaceChannelMessagesResponse,
  ListWorkspaceChannelsResponse,
  CreateWorkspaceChannelMessageRequest,
  WorkspaceChannel,
  WorkspaceChannelMessage,
  WorkspaceChannelMessageCacheEntry,
  WorkspaceChannelMessageCursor,
  WorkspaceChannelMessagesCache,
  WorkspaceChannelMessagesData,
  WorkspaceChannelMessagesInfiniteData,
} from "../types/channel";

function sameMessageRequest(
  left: WorkspaceChannelMessageCacheEntry,
  right: WorkspaceChannelMessage,
): boolean {
  return (
    left.workspace_id === right.workspace_id &&
    left.channel_id === right.channel_id &&
    left.author_type === right.author_type &&
    left.author_id === right.author_id &&
    left.content === right.content &&
    left.parent_id === right.parent_id &&
    left.quoted_message_id === right.quoted_message_id
  );
}

function sameValue(
  left: WorkspaceChannelMessageCacheEntry,
  right: WorkspaceChannelMessageCacheEntry,
): boolean {
  const keys = new Set([
    ...Object.keys(left),
    ...Object.keys(right),
  ] as (keyof WorkspaceChannelMessageCacheEntry)[]);
  for (const key of keys) {
    if (left[key] !== right[key]) return false;
  }
  return true;
}

function mergeMessage(
  existing: WorkspaceChannelMessageCacheEntry,
  incoming: WorkspaceChannelMessageCacheEntry,
): WorkspaceChannelMessageCacheEntry {
  // A server response or WS event is the authoritative replacement for a
  // local placeholder. Never let a late optimistic write downgrade it.
  if (existing.optimistic && !incoming.optimistic) return incoming;
  if (!existing.optimistic && incoming.optimistic) return existing;

  const merged = { ...existing, ...incoming };
  return sameValue(existing, merged) ? existing : merged;
}

function compareMessages(
  left: WorkspaceChannelMessageCacheEntry,
  right: WorkspaceChannelMessageCacheEntry,
): number {
  const leftTime = Date.parse(left.created_at);
  const rightTime = Date.parse(right.created_at);
  if (Number.isFinite(leftTime) && Number.isFinite(rightTime) && leftTime !== rightTime) {
    return leftTime - rightTime;
  }
  const byTimestamp = left.created_at.localeCompare(right.created_at);
  return byTimestamp || left.id.localeCompare(right.id);
}

function mergeMessageLists(
  existing: readonly WorkspaceChannelMessageCacheEntry[],
  incoming: readonly WorkspaceChannelMessageCacheEntry[],
): WorkspaceChannelMessageCacheEntry[] {
  const next = [...existing];
  let changed = false;

  for (const message of incoming) {
    const byId = next.findIndex((item) => item.id === message.id);
    if (byId >= 0) {
      const merged = mergeMessage(next[byId]!, message);
      if (merged !== next[byId]) {
        next[byId] = merged;
        changed = true;
      }
      continue;
    }

    // The Go message endpoint does not currently expose a client mutation id.
    // Match the pending placeholder on the complete request identity so an
    // HTTP response or WS echo cannot leave a second copy in the transcript.
    const optimisticIndex = next.findIndex(
      (item) => item.optimistic === true && sameMessageRequest(item, message),
    );
    if (optimisticIndex >= 0) {
      next[optimisticIndex] = message;
    } else {
      next.push(message);
    }
    changed = true;
  }

  if (!changed) return existing as WorkspaceChannelMessageCacheEntry[];
  next.sort(compareMessages);
  return next;
}

function mergeWorkspaceChannelMessagePage(
  existing: WorkspaceChannelMessagesCache | undefined,
  incoming: ListWorkspaceChannelMessagesResponse,
): WorkspaceChannelMessagesCache {
  const messages = mergeMessageLists(existing?.messages ?? [], incoming.messages);
  return {
    ...(existing ?? {}),
    ...incoming,
    messages,
  };
}

/**
 * Cursor windows should not overlap, but a refetch can race an optimistic or
 * realtime write while a row moves between windows. Remove only the duplicate
 * optimistic copy; keep the server row as the authoritative entry.
 */
function deduplicateInfiniteMessagePages(
  pages: WorkspaceChannelMessagesCache[],
): WorkspaceChannelMessagesCache[] {
  const authoritative = pages.flatMap((page) =>
    page.messages.filter((message) => message.optimistic !== true),
  );
  const authoritativeIds = new Set(authoritative.map((message) => message.id));
  const seenIds = new Set<string>();
  let changed = false;

  const nextPages = pages.map((page) => {
    const messages = page.messages.filter((message) => {
      if (
        message.optimistic === true &&
        (authoritativeIds.has(message.id) ||
          authoritative.some((serverMessage) =>
            sameMessageRequest(message, serverMessage),
          ))
      ) {
        changed = true;
        return false;
      }
      if (seenIds.has(message.id)) {
        changed = true;
        return false;
      }
      seenIds.add(message.id);
      return true;
    });
    return messages.length === page.messages.length ? page : { ...page, messages };
  });

  return changed ? nextPages : pages;
}

/**
 * Merge a list response with rows already written by an optimistic mutation
 * or realtime event. A refetch is not allowed to erase a message that arrived
 * while it was in flight; this is the same race protection used by the chat
 * message cache. Server rows win when the id is present in both lists.
 */
export function mergeWorkspaceChannelMessageResponses(
  existing: ListWorkspaceChannelMessagesResponse | undefined,
  incoming: ListWorkspaceChannelMessagesResponse,
): WorkspaceChannelMessagesCache {
  return mergeWorkspaceChannelMessagePage(existing, incoming);
}

/**
 * Preserve optimistic/realtime rows in every already-loaded infinite page
 * while React Query refetches the same cursor windows after reconnect.
 */
export function mergeWorkspaceChannelMessageInfiniteData(
  existing:
    | InfiniteData<
        ListWorkspaceChannelMessagesResponse,
        WorkspaceChannelMessageCursor | null
      >
    | undefined,
  incoming: InfiniteData<
    ListWorkspaceChannelMessagesResponse,
    WorkspaceChannelMessageCursor | null
  >,
): InfiniteData<
  WorkspaceChannelMessagesCache,
  WorkspaceChannelMessageCursor | null
> {
  return {
    ...incoming,
    pages: deduplicateInfiniteMessagePages(incoming.pages.map((page, index) =>
      mergeWorkspaceChannelMessagePage(existing?.pages[index], page),
    )),
  };
}

export function upsertWorkspaceChannelToCache(
  queryClient: QueryClient,
  workspaceId: string,
  channel: WorkspaceChannel,
  options: { seedIfMissing?: boolean } = {},
): void {
  if (!workspaceId || !channel.id || channel.workspace_id !== workspaceId) return;
  const { seedIfMissing = false } = options;
  queryClient.setQueryData<ListWorkspaceChannelsResponse | undefined>(
    channelKeys.list(workspaceId),
    (current) => {
      if (!current && !seedIfMissing) return current;
      if (!current) return { channels: [channel] };
      const index = current.channels.findIndex((item) => item.id === channel.id);
      if (index < 0) return { ...current, channels: [...current.channels, channel] };
      if (current.channels[index] === channel) return current;
      return {
        ...current,
        channels: current.channels.map((item, itemIndex) =>
          itemIndex === index ? channel : item,
        ),
      };
    },
  );
}

export function createOptimisticWorkspaceChannelMessage(
  workspaceId: string,
  channelId: string,
  input: Pick<
    CreateWorkspaceChannelMessageRequest,
    "author_type" | "author_id" | "content" | "parent_id" | "quoted_message_id"
  >,
): WorkspaceChannelMessageCacheEntry {
  const timestamp = new Date().toISOString();
  return {
    id: `optimistic-channel-message-${Date.now()}-${Math.random().toString(36).slice(2)}`,
    workspace_id: workspaceId,
    channel_id: channelId,
    author_type: input.author_type,
    author_id: input.author_id,
    content: input.content,
    parent_id: input.parent_id ?? null,
    quoted_message_id: input.quoted_message_id ?? null,
    created_at: timestamp,
    updated_at: timestamp,
    optimistic: true,
  };
}

export function upsertWorkspaceChannelMessageToCache(
  queryClient: QueryClient,
  workspaceId: string,
  channelId: string,
  message: WorkspaceChannelMessageCacheEntry,
  options: { seedIfMissing?: boolean } = {},
): void {
  if (
    !workspaceId ||
    !channelId ||
    message.workspace_id !== workspaceId ||
    message.channel_id !== channelId
  ) {
    return;
  }
  const { seedIfMissing = false } = options;
  queryClient.setQueryData<WorkspaceChannelMessagesData | undefined>(
    channelKeys.messages(workspaceId, channelId),
    (current) => {
      if (!current && !seedIfMissing) return current;
      if (!current) {
        return {
          pages: [
            {
              messages: [message],
              limit: 50,
              has_more: false,
              next_cursor: null,
            },
          ],
          pageParams: [null],
        } satisfies WorkspaceChannelMessagesInfiniteData;
      }
      if ("pages" in current) {
        const targetPageIndex = current.pages.findIndex((page) =>
          page.messages.some(
            (item) =>
              item.id === message.id ||
              (item.optimistic === true && sameMessageRequest(item, message)),
          ),
        );
        const firstPage = current.pages[0] ?? { messages: [] };
        const pageIndex = targetPageIndex >= 0 ? targetPageIndex : 0;
        return {
          ...current,
          pageParams: current.pageParams.length > 0 ? current.pageParams : [null],
          pages:
            current.pages.length === 0
              ? [{ ...firstPage, messages: [message] }]
              : current.pages.map((page, index) =>
                  index === pageIndex
                    ? { ...page, messages: mergeMessageLists(page.messages, [message]) }
                    : page,
                ),
        };
      }
      const base: WorkspaceChannelMessagesCache = current;
      return {
        ...base,
        messages: mergeMessageLists(base.messages, [message]),
      };
    },
  );
}

export function removeWorkspaceChannelMessageFromCache(
  queryClient: QueryClient,
  workspaceId: string,
  channelId: string,
  messageId: string,
): void {
  queryClient.setQueryData<WorkspaceChannelMessagesData | undefined>(
    channelKeys.messages(workspaceId, channelId),
    (current) => {
      if (!current) return current;
      if ("pages" in current) {
        let changed = false;
        const pages = current.pages.map((page) => {
          const messages = page.messages.filter((message) => message.id !== messageId);
          if (messages.length === page.messages.length) return page;
          changed = true;
          return { ...page, messages };
        });
        return changed ? { ...current, pages } : current;
      }
      const messages = current.messages.filter((message) => message.id !== messageId);
      return messages.length === current.messages.length
        ? current
        : { ...current, messages };
    },
  );
}
