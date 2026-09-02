import { describe, expect, it } from "vitest";
import type { WorkspaceChannelMessage } from "@patchbay/core/types";
import {
  channelSlugFromName,
  flattenChannelMessagePages,
  formatChannelDate,
} from "./channels-page";

describe("channel page helpers", () => {
  it("derives a stable slug from a channel name", () => {
    expect(channelSlugFromName("  Product planning!  ")).toBe("product-planning");
    expect(channelSlugFromName("路线 2026")).toBe("路线-2026");
  });

  it("formats valid dates and ignores malformed server timestamps", () => {
    expect(formatChannelDate("2026-09-02T12:34:00Z", "en-US")).not.toBe("");
    expect(formatChannelDate("not-a-date", "en-US")).toBe("");
  });

  it("renders older cursor pages before the newest page", () => {
    const older = { id: "older" } as WorkspaceChannelMessage;
    const newer = { id: "newer" } as WorkspaceChannelMessage;

    expect(
      flattenChannelMessagePages([
        { messages: [newer] },
        { messages: [older] },
      ]).map((message) => message.id),
    ).toEqual(["older", "newer"]);
  });
});
