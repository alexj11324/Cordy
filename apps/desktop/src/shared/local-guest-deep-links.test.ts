// @vitest-environment node

import { describe, expect, it } from "vitest";
import { MAIN_RENDERER_MESSAGE_CHANNELS } from "./main-renderer-messages";
import type { LocalGuestMode } from "./local-guest";
import {
  CLOUD_MAIN_RENDERER_CHANNELS,
  MAIN_RENDERER_CHANNEL_SCOPES,
  deepLinkDisposition,
} from "./local-guest-deep-links";

const MODES: LocalGuestMode[] = ["undecided", "guest", "cloud"];

describe("main renderer channel classification", () => {
  it("classifies every channel main can send to the renderer", () => {
    // The guard against the real regression: someone adds a new deep link
    // channel and forgets the Guest gate, so it silently defaults to
    // deliverable. Adding a channel without a scope fails here (and at
    // compile time, because the scope table is a total Record).
    for (const channel of MAIN_RENDERER_MESSAGE_CHANNELS) {
      expect(MAIN_RENDERER_CHANNEL_SCOPES[channel]).toMatch(/^(cloud|local)$/);
    }
    expect(Object.keys(MAIN_RENDERER_CHANNEL_SCOPES).sort()).toEqual(
      [...MAIN_RENDERER_MESSAGE_CHANNELS].sort(),
    );
  });

  it("treats the auth, invitation and issue deep links as cloud-scoped", () => {
    expect(CLOUD_MAIN_RENDERER_CHANNELS).toEqual(
      expect.arrayContaining([
        "auth:token", // patchbay://auth/callback?token=…
        "invite:open", // patchbay://invite/<id>
        "inbox:open", // notification click → cloud issue
      ]),
    );
  });
});

describe("deep link disposition", () => {
  it("rejects every cloud channel while Guest is active", () => {
    for (const channel of CLOUD_MAIN_RENDERER_CHANNELS) {
      expect(deepLinkDisposition(channel, "guest")).toBe("reject");
    }
  });

  it("defers, never delivers, a cloud channel before a mode is decided", () => {
    for (const channel of CLOUD_MAIN_RENDERER_CHANNELS) {
      expect(deepLinkDisposition(channel, "undecided")).toBe("defer");
    }
  });

  it("delivers cloud channels only in cloud mode", () => {
    for (const channel of CLOUD_MAIN_RENDERER_CHANNELS) {
      for (const mode of MODES) {
        expect(deepLinkDisposition(channel, mode) === "deliver").toBe(
          mode === "cloud",
        );
      }
    }
  });

  it("never returns deliver for a cloud channel outside cloud mode", () => {
    // Stated separately from the table above so a future channel marked
    // "local" by mistake cannot make this suite vacuously pass.
    const nonCloud: LocalGuestMode[] = ["guest", "undecided"];
    for (const channel of MAIN_RENDERER_MESSAGE_CHANNELS) {
      if (MAIN_RENDERER_CHANNEL_SCOPES[channel] !== "cloud") continue;
      for (const mode of nonCloud) {
        expect(deepLinkDisposition(channel, mode)).not.toBe("deliver");
      }
    }
  });
});
