import { describe, expect, it } from "vitest";
import {
  WorkspaceChannelListResponseSchema,
  WorkspaceChannelMessageListResponseSchema,
  channelSlugFromName,
  parseWorkspaceChannelCreatedEvent,
  parseWorkspaceChannelMessageEvent,
} from "./channel-types";

const CHANNEL = {
  id: "channel-1",
  workspace_id: "workspace-1",
  name: "Team updates",
  slug: "team-updates",
  description: null,
  created_by: "user-1",
  archived_at: null,
  created_at: "2026-09-02T00:00:00Z",
  updated_at: "2026-09-02T00:00:00Z",
};

const MESSAGE = {
  id: "message-1",
  workspace_id: "workspace-1",
  channel_id: "channel-1",
  author_type: "member",
  author_id: "user-1",
  content: "Hello",
  parent_id: null,
  quoted_message_id: null,
  created_at: "2026-09-02T00:00:00Z",
  updated_at: "2026-09-02T00:00:00Z",
};

describe("workspace channel response compatibility", () => {
  it("keeps valid channel rows and drops malformed rows independently", () => {
    const parsed = WorkspaceChannelListResponseSchema.parse({
      channels: [CHANNEL, { id: 42 }, { ...CHANNEL, id: "channel-2" }],
    });

    expect(parsed.channels).toHaveLength(2);
    expect(parsed.channels[0].description).toBe("");
    expect(parsed.channels[1].id).toBe("channel-2");
  });

  it("accepts the old messages-only Go envelope", () => {
    const parsed = WorkspaceChannelMessageListResponseSchema.parse({
      messages: [MESSAGE],
    });

    expect(parsed).toMatchObject({
      messages: [MESSAGE],
      has_more: false,
      next_cursor: null,
    });
  });

  it("accepts a valid cursor and terminates safely on an invalid cursor", () => {
    const valid = WorkspaceChannelMessageListResponseSchema.parse({
      messages: [MESSAGE],
      limit: 50,
      has_more: true,
      next_cursor: {
        created_at: "2026-09-01T23:59:00Z",
        id: "message-0",
      },
    });
    expect(valid.has_more).toBe(true);
    expect(valid.next_cursor).toEqual({
      created_at: "2026-09-01T23:59:00Z",
      id: "message-0",
    });

    const invalid = WorkspaceChannelMessageListResponseSchema.parse({
      messages: [MESSAGE],
      has_more: true,
      next_cursor: { created_at: "not-a-date", id: "message-0" },
    });
    expect(invalid.has_more).toBe(false);
    expect(invalid.next_cursor).toBeNull();
  });
});

describe("workspace channel event parsing", () => {
  it("handles direct and wrapped publisher payloads", () => {
    expect(parseWorkspaceChannelCreatedEvent({ channel: CHANNEL })?.id).toBe(
      "channel-1",
    );
    expect(
      parseWorkspaceChannelMessageEvent({ message: MESSAGE })?.id,
    ).toBe("message-1");
    expect(
      parseWorkspaceChannelMessageEvent({
        workspace_id: MESSAGE.workspace_id,
        channel_id: MESSAGE.channel_id,
        message: {
          ...MESSAGE,
          workspace_id: undefined,
          channel_id: undefined,
        },
      })?.workspace_id,
    ).toBe("workspace-1");
  });

  it("rejects an incomplete message before it can enter a cache", () => {
    expect(
      parseWorkspaceChannelMessageEvent({
        ...MESSAGE,
        content: "   ",
      }),
    ).toBeNull();
  });
});

describe("channelSlugFromName", () => {
  it("preserves Unicode letters and numbers while normalizing separators", () => {
    expect(channelSlugFromName("  发布 计划 / 第 2 版  ")).toBe(
      "发布-计划-第-2-版",
    );
    expect(channelSlugFromName("Team   Updates!")).toBe("team-updates");
  });
});
