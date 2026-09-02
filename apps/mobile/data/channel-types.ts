import { z } from "zod";

/**
 * Mobile-local channel contract.
 *
 * The shared Web/Desktop channel package is landing in a different write set.
 * Keeping this contract local lets the mobile app stay buildable against the
 * existing Go snapshot while accepting the additive cursor fields used by the
 * newer server. The shapes intentionally match the shared contract exactly so
 * this file can be replaced by a type-only import once that package is merged.
 */
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
  author_type: string;
  author_id: string;
  content: string;
  parent_id: string | null;
  quoted_message_id: string | null;
  created_at: string;
  updated_at: string;
};

export type WorkspaceChannelMessageCursor = {
  created_at: string;
  id: string;
};

export type ListWorkspaceChannelMessagesParams = {
  before?: WorkspaceChannelMessageCursor | null;
  limit?: number;
};

export type ListWorkspaceChannelsResponse = {
  channels: WorkspaceChannel[];
};

export type ListWorkspaceChannelMessagesResponse = {
  messages: WorkspaceChannelMessage[];
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
  content: string;
  parent_id?: string | null;
  quoted_message_id?: string | null;
};

export type WorkspaceChannelMessageCacheEntry = WorkspaceChannelMessage & {
  optimistic?: boolean;
};

const ChannelRowSchema = z
  .object({
    id: z.string().min(1),
    workspace_id: z.string().default(""),
    name: z.string().default(""),
    slug: z.string().default(""),
    description: z.string().nullish().transform((value) => value ?? ""),
    created_by: z.string().default(""),
    archived_at: z.string().nullable().default(null),
    created_at: z.string().default(""),
    updated_at: z.string().default(""),
  })
  .loose();

const MessageRowSchema = z
  .object({
    id: z.string().min(1),
    workspace_id: z.string().default(""),
    channel_id: z.string().default(""),
    author_type: z.string().default("member"),
    author_id: z.string().default(""),
    content: z.string().default(""),
    parent_id: z.string().nullable().default(null),
    quoted_message_id: z.string().nullable().default(null),
    created_at: z.string().default(""),
    updated_at: z.string().default(""),
  })
  .loose();

const CursorSchema = z
  .object({
    created_at: z.string(),
    id: z.string().min(1),
  })
  .loose()
  .refine((value) => Number.isFinite(Date.parse(value.created_at)), {
    message: "cursor timestamp must be a valid date",
  });

/**
 * Parse list rows independently. A future server field is preserved by
 * `.loose()`, while one malformed row is dropped instead of emptying a whole
 * workspace list. Required identity fields still protect cache writes.
 */
export const WorkspaceChannelListResponseSchema = z
  .object({ channels: z.array(z.unknown()).default([]) })
  .loose()
  .transform((value): ListWorkspaceChannelsResponse => ({
    channels: value.channels.flatMap((row) => {
      const parsed = ChannelRowSchema.safeParse(row);
      return parsed.success ? [parsed.data as WorkspaceChannel] : [];
    }),
  }));

export const EMPTY_WORKSPACE_CHANNEL_LIST_RESPONSE: ListWorkspaceChannelsResponse = {
  channels: [],
};

/** The old Go snapshot omits cursor metadata; those fields remain optional. */
export const WorkspaceChannelMessageListResponseSchema = z
  .object({
    messages: z.array(z.unknown()).default([]),
    limit: z.number().int().min(1).max(100).optional(),
    has_more: z.boolean().optional(),
    next_cursor: z.unknown().optional(),
  })
  .loose()
  .transform((value): ListWorkspaceChannelMessagesResponse => {
    const messages = value.messages.flatMap((row) => {
      const parsed = MessageRowSchema.safeParse(row);
      if (!parsed.success || !parsed.data.workspace_id || !parsed.data.channel_id) {
        return [];
      }
      if (!parsed.data.content.trim() || !parsed.data.author_id) return [];
      return [parsed.data as WorkspaceChannelMessage];
    });
    const cursor = value.has_more
      ? CursorSchema.safeParse(value.next_cursor)
      : { success: false as const };
    return {
      messages,
      ...(value.limit === undefined ? {} : { limit: value.limit }),
      has_more: cursor.success,
      next_cursor: cursor.success ? cursor.data : null,
    };
  });

export const EMPTY_WORKSPACE_CHANNEL_MESSAGE_LIST_RESPONSE: ListWorkspaceChannelMessagesResponse = {
  messages: [],
  has_more: false,
  next_cursor: null,
};

export const WorkspaceChannelSchema = ChannelRowSchema;
export const WorkspaceChannelMessageSchema = MessageRowSchema;

export const EMPTY_WORKSPACE_CHANNEL: WorkspaceChannel = {
  id: "",
  workspace_id: "",
  name: "",
  slug: "",
  description: "",
  created_by: "",
  archived_at: null,
  created_at: "",
  updated_at: "",
};

export const EMPTY_WORKSPACE_CHANNEL_MESSAGE: WorkspaceChannelMessage = {
  id: "",
  workspace_id: "",
  channel_id: "",
  author_type: "member",
  author_id: "",
  content: "",
  parent_id: null,
  quoted_message_id: null,
  created_at: "",
  updated_at: "",
};

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null
    ? (value as Record<string, unknown>)
    : null;
}

/** Accept both the direct event row and `{ channel: row }` publishers. */
export function parseWorkspaceChannelCreatedEvent(
  payload: unknown,
): WorkspaceChannel | null {
  const record = asRecord(payload);
  const parsed = ChannelRowSchema.safeParse(record?.channel ?? payload);
  return parsed.success ? (parsed.data as WorkspaceChannel) : null;
}

/** Accept both the direct event row and `{ message: row }` publishers. */
export function parseWorkspaceChannelMessageEvent(
  payload: unknown,
): WorkspaceChannelMessage | null {
  const outer = asRecord(payload);
  const candidate = outer?.message ?? payload;
  const record = asRecord(candidate);
  if (!record) return null;

  // Some publishers put routing ids beside a reduced message object. Fill
  // those two fields before parsing, while still rejecting incomplete rows.
  const merged = {
    ...record,
    workspace_id: record.workspace_id ?? outer?.workspace_id,
    channel_id: record.channel_id ?? outer?.channel_id,
  };
  const parsed = MessageRowSchema.safeParse(merged);
  if (
    !parsed.success ||
    !parsed.data.workspace_id ||
    !parsed.data.channel_id ||
    !parsed.data.author_id ||
    !parsed.data.content.trim()
  ) {
    return null;
  }
  return parsed.data as WorkspaceChannelMessage;
}

/** Convert a human name into the slug accepted by both channel handlers. */
export function channelSlugFromName(name: string): string {
  return name
    .trim()
    .toLocaleLowerCase()
    .replace(/\s+/g, "-")
    .replace(/[^\p{L}\p{N}-]/gu, "")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "");
}

export function formatChannelTimestamp(value: string, locale: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  try {
    return new Intl.DateTimeFormat(locale, {
      dateStyle: "short",
      timeStyle: "short",
    }).format(date);
  } catch {
    return "";
  }
}
