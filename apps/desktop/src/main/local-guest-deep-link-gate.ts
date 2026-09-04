import type { LocalGuestMode } from "../shared/local-guest";
import {
  CLOUD_MAIN_RENDERER_CHANNELS,
  deepLinkDisposition,
} from "../shared/local-guest-deep-links";
import type { MainRendererMessageChannel } from "../shared/main-renderer-messages";

/**
 * A deferred deep link is an unconsumed cloud intent. The cap keeps a hostile
 * or looping URL handler from growing main's heap while no mode is decided;
 * the newest intent is the one the user just triggered, so the oldest is
 * dropped first.
 */
export const MAX_DEFERRED_DEEP_LINKS = 16;

export type DeepLinkMessageQueue = {
  enqueue(
    channel: MainRendererMessageChannel,
    payload: unknown,
    send: (channel: MainRendererMessageChannel, payload: unknown) => void,
  ): void;
  clear(channel: MainRendererMessageChannel): void;
};

type DeferredDeepLink = {
  channel: MainRendererMessageChannel;
  payload: unknown;
};

/**
 * The single place where a cloud deep link is allowed to become renderer
 * traffic.
 *
 * Guest is Desktop-only and local-only, so no cloud payload may reach a
 * renderer that is in — or is about to enter — Guest mode. The decision is
 * taken from the main-owned mode (`getMode`), never from anything the
 * renderer reports, and rejection also *clears* whatever is already queued:
 * a payload that survives in the queue would be delivered the moment the user
 * later switched to cloud, which is a delayed leak rather than a prevented one.
 */
export class GuestDeepLinkGate {
  readonly #queue: DeepLinkMessageQueue;
  readonly #send: (
    channel: MainRendererMessageChannel,
    payload: unknown,
  ) => void;
  readonly #getMode: () => LocalGuestMode;
  #deferred: DeferredDeepLink[] = [];

  constructor(
    queue: DeepLinkMessageQueue,
    send: (channel: MainRendererMessageChannel, payload: unknown) => void,
    getMode: () => LocalGuestMode,
  ) {
    this.#queue = queue;
    this.#send = send;
    this.#getMode = getMode;
  }

  /** Returns true only when the payload actually reached the renderer queue. */
  dispatch(channel: MainRendererMessageChannel, payload: unknown): boolean {
    switch (deepLinkDisposition(channel, this.#getMode())) {
      case "deliver":
        this.#queue.enqueue(channel, payload, this.#send);
        return true;
      case "defer":
        if (this.#deferred.length >= MAX_DEFERRED_DEEP_LINKS) {
          this.#deferred.shift();
        }
        this.#deferred.push({ channel, payload });
        return false;
      case "reject":
        this.rejectCloudTraffic();
        return false;
    }
  }

  /**
   * Applies a main-owned mode transition. Returns true when deferred cloud
   * intents were released, so the caller can raise the window for them.
   */
  applyMode(mode: LocalGuestMode): boolean {
    if (mode === "cloud") {
      const released = this.#deferred;
      this.#deferred = [];
      for (const { channel, payload } of released) {
        this.#queue.enqueue(channel, payload, this.#send);
      }
      return released.length > 0;
    }
    // "guest" and "undecided" are both non-cloud: a session that left cloud
    // must not keep a token or invite alive for the next one to inherit.
    this.rejectCloudTraffic();
    return false;
  }

  /** Drops deferred intents and every cloud payload already queued. */
  rejectCloudTraffic(): void {
    this.#deferred = [];
    for (const channel of CLOUD_MAIN_RENDERER_CHANNELS) {
      this.#queue.clear(channel);
    }
  }

  /** True while an explicit cloud intent is waiting for a mode decision. */
  hasDeferred(): boolean {
    return this.#deferred.length > 0;
  }
}
