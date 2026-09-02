// @vitest-environment node

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { LocalGuestMode } from "../shared/local-guest";
import { CLOUD_MAIN_RENDERER_CHANNELS } from "../shared/local-guest-deep-links";
import {
  MainRendererMessageQueue,
  type MainRendererMessageChannel,
} from "../shared/main-renderer-messages";
import {
  GuestDeepLinkGate,
  MAX_DEFERRED_DEEP_LINKS,
} from "./local-guest-deep-link-gate";

const AUTH_TOKEN = "eyJhbGciOiJIUzI1NiJ9.stolen.token";

/**
 * The real queue, not a stub: half the contract is that rejection clears what
 * the queue is already holding, which a stub could not prove.
 */
function createGate(initialMode: LocalGuestMode) {
  const queue = new MainRendererMessageQueue();
  const delivered: Array<{
    channel: MainRendererMessageChannel;
    payload: unknown;
  }> = [];
  let mode = initialMode;
  const gate = new GuestDeepLinkGate(
    queue,
    (channel, payload) => delivered.push({ channel, payload }),
    () => mode,
  );
  const setMode = (next: LocalGuestMode) => {
    mode = next;
    return gate.applyMode(next);
  };
  // The renderer signals readiness per channel; do it up front so a delivered
  // payload lands in `delivered` rather than sitting pending.
  const markRendererReady = () => {
    for (const channel of CLOUD_MAIN_RENDERER_CHANNELS) {
      queue.setReady(channel, true, (c, p) => delivered.push({ channel: c, payload: p }));
    }
  };
  return { gate, queue, delivered, setMode, markRendererReady };
}

describe("GuestDeepLinkGate", () => {
  let gateFixture: ReturnType<typeof createGate>;

  beforeEach(() => {
    gateFixture = createGate("undecided");
  });

  it("drops every cloud deep link while Guest is active", () => {
    const { gate, delivered, setMode, markRendererReady } = createGate("guest");
    markRendererReady();

    for (const channel of CLOUD_MAIN_RENDERER_CHANNELS) {
      expect(gate.dispatch(channel, AUTH_TOKEN)).toBe(false);
    }

    expect(delivered).toEqual([]);
    // And they stay dropped: switching to cloud later must not resurrect a
    // token that arrived while the user was in Guest.
    setMode("cloud");
    expect(delivered).toEqual([]);
  });

  it("clears cloud payloads already queued when Guest takes over", () => {
    const { gate, delivered, setMode, markRendererReady } = gateFixture;

    // Arrives while undecided, so it is held rather than delivered.
    expect(gate.dispatch("auth:token", AUTH_TOKEN)).toBe(false);
    expect(gate.hasDeferred()).toBe(true);

    // The user picks Guest. The held intent must not survive.
    expect(setMode("guest")).toBe(false);
    expect(gate.hasDeferred()).toBe(false);

    markRendererReady();
    setMode("cloud");
    expect(delivered).toEqual([]);
  });

  it("releases a deferred deep link once main itself enters cloud mode", () => {
    const { gate, delivered, setMode, markRendererReady } = gateFixture;
    markRendererReady();

    gate.dispatch("auth:token", AUTH_TOKEN);
    gate.dispatch("invite:open", "invitation-42");
    expect(delivered).toEqual([]);

    expect(setMode("cloud")).toBe(true);
    expect(delivered).toEqual([
      { channel: "auth:token", payload: AUTH_TOKEN },
      { channel: "invite:open", payload: "invitation-42" },
    ]);
    expect(gate.hasDeferred()).toBe(false);
  });

  it("delivers immediately once cloud mode is established", () => {
    const { gate, delivered, markRendererReady } = createGate("cloud");
    markRendererReady();

    expect(gate.dispatch("inbox:open", { slug: "acme", itemId: "i1" })).toBe(
      true,
    );
    expect(delivered).toEqual([
      { channel: "inbox:open", payload: { slug: "acme", itemId: "i1" } },
    ]);
  });

  it("clears cloud traffic when cloud services are torn down", () => {
    const { gate, delivered, setMode, markRendererReady } = createGate("cloud");

    // Queued but not yet consumed by the renderer.
    gate.dispatch("auth:token", AUTH_TOKEN);
    gate.rejectCloudTraffic();

    markRendererReady();
    setMode("cloud");
    expect(delivered).toEqual([]);
  });

  it("bounds how many intents it will hold while undecided", () => {
    const { gate, delivered, setMode, markRendererReady } = gateFixture;
    markRendererReady();

    for (let index = 0; index < MAX_DEFERRED_DEEP_LINKS + 5; index += 1) {
      gate.dispatch("invite:open", `invitation-${index}`);
    }
    setMode("cloud");

    expect(delivered).toHaveLength(MAX_DEFERRED_DEEP_LINKS);
    // The newest intent — the one the user just triggered — survives.
    expect(delivered.at(-1)?.payload).toBe(
      `invitation-${MAX_DEFERRED_DEEP_LINKS + 4}`,
    );
  });

  it("asks main for the mode on every dispatch", () => {
    // A cached mode is how a stale cloud decision leaks into a Guest session.
    const getMode = vi.fn<() => LocalGuestMode>(() => "cloud");
    const queue = new MainRendererMessageQueue();
    const gate = new GuestDeepLinkGate(queue, () => {}, getMode);

    gate.dispatch("auth:token", AUTH_TOKEN);
    getMode.mockReturnValue("guest");
    expect(gate.dispatch("auth:token", AUTH_TOKEN)).toBe(false);
    expect(getMode).toHaveBeenCalledTimes(2);
  });
});
