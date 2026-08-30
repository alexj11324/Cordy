/**
 * Main-process messages that must wait until the main renderer has installed
 * the matching listener. A BrowserWindow finishing `loadURL` is not enough:
 * React effects subscribe later, so an eager deep link can otherwise vanish.
 */
export const MAIN_RENDERER_CHANNEL_STATE_CHANNEL =
  "main-renderer:channel-state";
export const MAIN_RENDERER_MESSAGE_ACK_CHANNEL =
  "main-renderer:message-ack";

export const MAIN_RENDERER_MESSAGE_CHANNELS = [
  "auth:handoff",
  "invite:open",
  "inbox:open",
  "settings:open",
] as const;

export type MainRendererMessageChannel =
  (typeof MAIN_RENDERER_MESSAGE_CHANNELS)[number];

export interface MainRendererChannelState {
  channel: MainRendererMessageChannel;
  ready: boolean;
}

const mainRendererMessageChannels = new Set<string>(
  MAIN_RENDERER_MESSAGE_CHANNELS,
);

export function parseMainRendererChannelState(
  value: unknown,
): MainRendererChannelState | null {
  if (!value || typeof value !== "object") return null;
  const candidate = value as Record<string, unknown>;
  if (
    typeof candidate.channel !== "string" ||
    !mainRendererMessageChannels.has(candidate.channel) ||
    typeof candidate.ready !== "boolean"
  ) {
    return null;
  }
  return {
    channel: candidate.channel as MainRendererMessageChannel,
    ready: candidate.ready,
  };
}

type SendMessage = (
  channel: MainRendererMessageChannel,
  payload: unknown,
) => void;

type AuthHandoffPayload = {
  code: string;
  state: string;
};

export type MainRendererMessageAcknowledgement = {
  channel: "auth:handoff";
  payload: AuthHandoffPayload;
};

function isAuthHandoffPayload(value: unknown): value is AuthHandoffPayload {
  if (!value || typeof value !== "object") return false;
  const candidate = value as Record<string, unknown>;
  return typeof candidate.code === "string" && typeof candidate.state === "string";
}

function isSameAuthHandoff(left: unknown, right: AuthHandoffPayload): boolean {
  return (
    isAuthHandoffPayload(left) &&
    left.code === right.code &&
    left.state === right.state
  );
}

export function parseMainRendererMessageAcknowledgement(
  value: unknown,
): MainRendererMessageAcknowledgement | null {
  if (!value || typeof value !== "object") return null;
  const candidate = value as Record<string, unknown>;
  if (candidate.channel !== "auth:handoff") return null;
  if (!isAuthHandoffPayload(candidate.payload)) return null;
  return {
    channel: "auth:handoff",
    payload: candidate.payload,
  };
}

/**
 * Holds renderer-bound messages until the specific React listener is ready.
 * Readiness is per BrowserWindow lifecycle; pending messages intentionally
 * survive a main-window close so recreating the window still delivers them.
 */
export class MainRendererMessageQueue {
  private readonly readyChannels = new Set<MainRendererMessageChannel>();
  private readonly pending = new Map<
    MainRendererMessageChannel,
    unknown[]
  >();

  enqueue(
    channel: MainRendererMessageChannel,
    payload: unknown,
    send: SendMessage,
  ): void {
    if (channel === "auth:handoff") {
      const queued = this.pending.get(channel) ?? [];
      queued.push(payload);
      this.pending.set(channel, queued);
      if (this.readyChannels.has(channel)) send(channel, payload);
      return;
    }

    if (this.readyChannels.has(channel)) {
      send(channel, payload);
      return;
    }
    const queued = this.pending.get(channel) ?? [];
    queued.push(payload);
    this.pending.set(channel, queued);
  }

  setReady(
    channel: MainRendererMessageChannel,
    ready: boolean,
    send: SendMessage,
  ): void {
    if (!ready) {
      this.readyChannels.delete(channel);
      return;
    }

    this.readyChannels.add(channel);
    const queued = this.pending.get(channel);
    if (!queued) return;
    for (const payload of queued) send(channel, payload);
    if (channel !== "auth:handoff") this.pending.delete(channel);
  }

  /** Retire an auth handoff only after its renderer has redeemed it. */
  acknowledge(channel: MainRendererMessageChannel, payload: unknown): void {
    if (channel !== "auth:handoff" || !isAuthHandoffPayload(payload)) return;
    const queued = this.pending.get(channel);
    if (!queued) return;
    const index = queued.findIndex((candidate) =>
      isSameAuthHandoff(candidate, payload),
    );
    if (index < 0) return;
    queued.splice(index, 1);
    if (queued.length === 0) this.pending.delete(channel);
  }

  /** Clear readiness when the main renderer is replaced, without losing work. */
  resetReady(): void {
    this.readyChannels.clear();
  }

  /** Drop messages that are no longer safe for the active account. */
  clear(channel: MainRendererMessageChannel): void {
    this.pending.delete(channel);
  }
}
