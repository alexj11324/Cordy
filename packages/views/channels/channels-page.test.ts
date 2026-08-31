import { describe, expect, it } from "vitest";
import { shouldSyncChannelUrl } from "./channels-page";

describe("shouldSyncChannelUrl", () => {
  it("does not restore the channel URL after navigation has left the page", () => {
    expect(
      shouldSyncChannelUrl("/acme/agents", "/acme/channels", null, "channel-1"),
    ).toBe(false);
  });

  it("adds the selected channel while the current route is still channels", () => {
    expect(
      shouldSyncChannelUrl("/acme/channels", "/acme/channels", null, "channel-1"),
    ).toBe(true);
  });
});
