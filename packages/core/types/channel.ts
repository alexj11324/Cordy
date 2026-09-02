export type WorkspaceChannelAuthorType = "member" | "agent";

/**
 * Channel realtime names are kept next to the channel contract until the
 * shared WS event registry registers these events. The current Go handler
 * already publishes them, so the channel adapter casts only at the subscribe
 * boundary without widening the shared union outside this stage's file scope.
 */
export const CHANNEL_CREATED_EVENT = "channel:created" as const;
export const CHANNEL_MESSAGE_EVENT = "channel:message" as const;

export type WorkspaceChannelEventType =
  | typeof CHANNEL_CREATED_EVENT
  | typeof CHANNEL_MESSAGE_EVENT;

export type WorkspaceChannel = {
  id: string;
  workspace_id: string;
  name: string;
  slug: string;
  description: string;
  created_by: string;
  archived_at: string | null;
  created_at: string;
  updated_at: string;
};

export type WorkspaceChannelMessage = {
  id: string;
  workspace_id: string;
  channel_id: string;
  /** The server currently accepts `member` and `agent`; keep responses open for additive actor types. */
  author_type: string;
  author_id: string;
  content: string;
  parent_id: string | null;
  quoted_message_id: string | null;
  created_at: string;
  updated_at: string;
};

/** Stable key used by cursor-based channel message APIs. */
export type WorkspaceChannelMessageCursor = {
  created_at: string;
  id: string;
};

export type ListWorkspaceChannelMessagesParams = {
  /** Cursor for the newest row already loaded; the Go API returns older rows. */
  before?: WorkspaceChannelMessageCursor | null;
  /** The Go handler accepts values from 1 through 100. */
  limit?: number;
};

/**
 * A message row held in the local cache may carry a client-only optimistic
 * marker. The marker is never sent to or read from the API.
 */
export type WorkspaceChannelMessageCacheEntry = WorkspaceChannelMessage & {
  optimistic?: boolean;
};

export type WorkspaceChannelMessagesCache = Omit<
  ListWorkspaceChannelMessagesResponse,
  "messages"
> & {
  messages: WorkspaceChannelMessageCacheEntry[];
};

export type WorkspaceChannelMessagesInfiniteData = {
  pages: WorkspaceChannelMessagesCache[];
  pageParams: Array<WorkspaceChannelMessageCursor | null>;
};

export type WorkspaceChannelMessagesData =
  | WorkspaceChannelMessagesCache
  | WorkspaceChannelMessagesInfiniteData;

export type ListWorkspaceChannelsResponse = {
  channels: WorkspaceChannel[];
};

export type ListWorkspaceChannelMessagesResponse = {
  messages: WorkspaceChannelMessage[];
  /** Cursor metadata returned by the Go handler for loading older pages. */
  limit?: number;
  has_more?: boolean;
  next_cursor?: WorkspaceChannelMessageCursor | null;
};

export type CreateWorkspaceChannelRequest = {
  slug: string;
  name?: string;
  description?: string;
};

export type CreateWorkspaceChannelMessageRequest = {
  author_type: WorkspaceChannelAuthorType;
  author_id: string;
  content: string;
  parent_id?: string | null;
  quoted_message_id?: string | null;
};

export type WorkspaceChannelCreatedEventPayload =
  | WorkspaceChannel
  | {
      channel: WorkspaceChannel;
      workspace_id?: string;
    };

export type WorkspaceChannelMessageEventPayload =
  | WorkspaceChannelMessage
  | {
      message: WorkspaceChannelMessage;
      channel_id?: string;
      workspace_id?: string;
    };

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null
    ? (value as Record<string, unknown>)
    : null;
}

function requiredString(record: Record<string, unknown>, key: string): string | null {
  const value = record[key];
  return typeof value === "string" && value.length > 0 ? value : null;
}

function nullableString(
  record: Record<string, unknown>,
  key: string,
  defaultValue?: string | null,
): string | null | undefined {
  const value = record[key];
  if (value === undefined) return defaultValue;
  return value === null || typeof value === "string" ? value : undefined;
}

function parseChannel(value: unknown): WorkspaceChannel | null {
  const record = asRecord(value);
  if (!record) return null;

  const id = requiredString(record, "id");
  const workspaceId = requiredString(record, "workspace_id");
  const name = requiredString(record, "name");
  const slug = requiredString(record, "slug");
  const description = typeof record.description === "string" ? record.description : null;
  const createdBy = requiredString(record, "created_by");
  const createdAt = requiredString(record, "created_at");
  const updatedAt = requiredString(record, "updated_at");
  const archivedAt = nullableString(record, "archived_at");
  if (
    !id ||
    !workspaceId ||
    !name ||
    !slug ||
    description === null ||
    !createdBy ||
    !createdAt ||
    !updatedAt ||
    archivedAt === undefined
  ) {
    return null;
  }

  return {
    id,
    workspace_id: workspaceId,
    name,
    slug,
    description,
    created_by: createdBy,
    archived_at: archivedAt,
    created_at: createdAt,
    updated_at: updatedAt,
  };
}

/** Drop malformed rows produced by the lenient API schema before list state is cached. */
export function normalizeWorkspaceChannelsResponse(
  input: unknown,
): ListWorkspaceChannelsResponse {
  const record = asRecord(input);
  const rawChannels = record?.channels;
  const channels = Array.isArray(rawChannels)
    ? rawChannels
        .map((channel) => parseChannel(channel))
        .filter((channel): channel is WorkspaceChannel => channel !== null)
    : [];
  return { channels };
}

/** Extract a complete channel from either the direct or wrapped event shape. */
export function parseWorkspaceChannelCreatedEvent(
  payload: unknown,
): WorkspaceChannel | null {
  const record = asRecord(payload);
  return parseChannel(record?.channel ?? payload);
}

/**
 * Extract a complete message from either the direct or wrapped event shape.
 * The wrapper fallbacks make the adapter tolerant of a publisher that keeps
 * routing fields beside a reduced message object, while still refusing to
 * write incomplete rows into the transcript cache.
 */
export function parseWorkspaceChannelMessageEvent(
  payload: unknown,
): WorkspaceChannelMessage | null {
  const outer = asRecord(payload);
  const record = asRecord(outer?.message ?? payload);
  if (!record) return null;

  const id = requiredString(record, "id");
  const workspaceId = requiredString(record, "workspace_id") ??
    (outer ? requiredString(outer, "workspace_id") : null);
  const channelId = requiredString(record, "channel_id") ??
    (outer ? requiredString(outer, "channel_id") : null);
  const authorType = requiredString(record, "author_type");
  const authorId = requiredString(record, "author_id");
  const content = typeof record.content === "string" ? record.content : null;
  const createdAt = requiredString(record, "created_at");
  const updatedAt = requiredString(record, "updated_at") ?? createdAt;
  const parentId = nullableString(record, "parent_id", null);
  const quotedMessageId = nullableString(record, "quoted_message_id", null);
  if (
    !id ||
    !workspaceId ||
    !channelId ||
    !authorType ||
    !authorId ||
    content === null ||
    content.trim().length === 0 ||
    !createdAt ||
    !updatedAt ||
    parentId === undefined ||
    quotedMessageId === undefined
  ) {
    return null;
  }

  return {
    id,
    workspace_id: workspaceId,
    channel_id: channelId,
    author_type: authorType,
    author_id: authorId,
    content,
    parent_id: parentId,
    quoted_message_id: quotedMessageId,
    created_at: createdAt,
    updated_at: updatedAt,
  };
}

function parseWorkspaceChannelMessageCursor(
  value: unknown,
): WorkspaceChannelMessageCursor | null {
  const record = asRecord(value);
  if (!record) return null;

  const createdAt = requiredString(record, "created_at");
  const id = requiredString(record, "id");
  if (!createdAt || !id || !Number.isFinite(Date.parse(createdAt))) return null;

  return { created_at: createdAt, id };
}

function parseWorkspaceChannelMessageLimit(value: unknown): number | undefined {
  return typeof value === "number" && Number.isInteger(value) && value >= 1 && value <= 100
    ? value
    : undefined;
}

/**
 * Normalize the lenient API response before it reaches the query cache. The
 * server's cursor fields are additive, so older responses remain valid; an
 * invalid cursor is treated as the end of the page rather than creating an
 * unsafe pagination loop.
 */
export function normalizeWorkspaceChannelMessagesResponse(
  input: unknown,
): ListWorkspaceChannelMessagesResponse {
  const record = asRecord(input);
  const rawMessages = record?.messages;
  const messages = Array.isArray(rawMessages)
    ? rawMessages
        .map((message) => parseWorkspaceChannelMessageEvent(message))
        .filter((message): message is WorkspaceChannelMessage => message !== null)
    : [];
  const limit = parseWorkspaceChannelMessageLimit(record?.limit);
  const hasMore = record?.has_more === true;
  const nextCursor = hasMore
    ? parseWorkspaceChannelMessageCursor(record?.next_cursor)
    : null;

  return {
    messages,
    ...(limit === undefined ? {} : { limit }),
    has_more: hasMore && nextCursor !== null,
    next_cursor: nextCursor,
  };
}
