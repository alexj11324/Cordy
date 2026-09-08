import { type Logger, noopLogger } from "../logger";
import type { WSEventType, WSMessage } from "../types/events";
import { parseWSFrame } from "./ws-schema";

type EventHandler = (payload: unknown, actorId?: string, actorType?: string) => void;

// Cap how much of an unparseable frame we put into the log. A malformed or
// rogue server can stream arbitrarily large garbage, and the warn handler may
// be a console / IPC bridge whose buffers we don't want to blow.
const UNPARSEABLE_LOG_MAX_CHARS = 200;

// Reconnect backoff parameters. A flat delay causes a thundering herd when many
// clients reconnect after a server restart; exponential backoff with jitter
// spreads the reconnection attempts over time. The client retries indefinitely
// (capped at RECONNECT_MAX_DELAY_MS) because the web/desktop UI does not yet
// expose a visible disconnected state or manual retry action.
const RECONNECT_BASE_DELAY_MS = 1_000;
const RECONNECT_MAX_DELAY_MS = 30_000;

function summarizeUnparseable(data: unknown): string {
  const text = typeof data === "string" ? data : String(data);
  if (text.length <= UNPARSEABLE_LOG_MAX_CHARS) return text;
  return `${text.slice(0, UNPARSEABLE_LOG_MAX_CHARS)}… (truncated, ${text.length} chars total)`;
}

/** Identifies the WS client to the server. Sent as `client_platform`,
 *  `client_version`, and `client_os` query parameters on the upgrade URL —
 *  browsers cannot set custom headers on WebSocket handshakes, so query
 *  params are the only portable channel. */
export interface WSClientIdentity {
  platform?: string;
  version?: string;
  os?: string;
}

export class WSClient {
  private ws: WebSocket | null = null;
  private generation = 0;

  get connectionGeneration() { return this.generation; }
  get subscriptionWorkspaceSlug() { return this.workspaceSlug; }
  private baseUrl: string;
  private token: string | null = null;
  private workspaceSlug: string | null = null;
  private cookieAuth = false;
  private identity: WSClientIdentity | undefined;
  private handlers = new Map<WSEventType, Set<EventHandler>>();
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectAttempt = 0;
  private hasConnectedBefore = false;
  // One-shot per connection. A non-conforming frame can repeat hundreds of
  // times per session, so we log the first drop and suppress the rest. Reset
  // on each connect() so a fresh connection logs once again.
  private badFrameLogged = false;
  private onReconnectCallbacks = new Set<() => void>();
  private anyHandlers = new Set<(msg: WSMessage) => void>();
  private logger: Logger;

  constructor(
    url: string,
    options?: {
      logger?: Logger;
      cookieAuth?: boolean;
      identity?: WSClientIdentity;
    },
  ) {
    this.baseUrl = url;
    this.logger = options?.logger ?? noopLogger;
    this.cookieAuth = options?.cookieAuth ?? false;
    this.identity = options?.identity;
  }

  setAuth(token: string | null, workspaceSlug: string) {
    this.token = token;
    this.workspaceSlug = workspaceSlug;
  }

  connect() {
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
    const previous = this.ws;
    this.ws = null;
    const generation = ++this.generation;
    if (previous) {
      previous.onopen = previous.onmessage = previous.onclose = previous.onerror = null;
      previous.close();
    }
    this.badFrameLogged = false;
    const url = new URL(this.baseUrl);
    // Token is never sent as a URL query parameter — it would be logged by
    // proxies, CDNs, and browser history.  In cookie mode the HttpOnly cookie
    // is sent automatically with the upgrade request.  In token mode the token
    // is delivered as the first WebSocket message after the connection opens.
    if (this.workspaceSlug)
      url.searchParams.set("workspace_slug", this.workspaceSlug);
    if (this.identity?.platform)
      url.searchParams.set("client_platform", this.identity.platform);
    if (this.identity?.version)
      url.searchParams.set("client_version", this.identity.version);
    if (this.identity?.os)
      url.searchParams.set("client_os", this.identity.os);

    const socket = new WebSocket(url.toString());
    this.ws = socket;
    const token = this.token;
    const isCurrent = () => this.ws === socket && this.generation === generation;
    let authenticated = false;
    const authenticate = () => {
      if (!isCurrent() || authenticated) return;
      authenticated = true;
      this.onAuthenticated();
    };

    socket.onopen = () => {
      if (!isCurrent()) return;
      if (!this.cookieAuth && token) {
        socket.send(
          JSON.stringify({ type: "auth", payload: { token } }),
        );
        return;
      }

      authenticate();
    };

    socket.onmessage = (event) => {
      if (!isCurrent()) return;
      let data: unknown;
      try {
        data = JSON.parse(event.data as string);
      } catch {
        this.logger.warn("ws: received unparseable message", summarizeUnparseable(event.data));
        return;
      }
      const result = parseWSFrame(data);
      if (result.kind === "invalid") {
        if (!this.badFrameLogged) {
          this.badFrameLogged = true;
          // Log paths/codes only. A rejected frame may contain private content.
          this.logger.warn("ws: dropping invalid frame", result.issues);
        }
        return;
      }
      if (result.kind === "unknown") return;
      if (result.kind === "auth") {
        authenticate();
        return;
      }
      const msg = result.message;
      this.logger.debug("received", msg.type);
      const eventHandlers = this.handlers.get(msg.type);
      if (eventHandlers) {
        for (const handler of eventHandlers) {
          if (!isCurrent()) return;
          this.invokeListener(msg.type, () => handler(msg.payload, msg.actor_id, msg.actor_type));
        }
      }
      for (const handler of this.anyHandlers) {
        if (!isCurrent()) return;
        this.invokeListener(msg.type, () => handler(msg));
      }
    };

    socket.onclose = () => {
      if (!isCurrent()) return;
      this.ws = null;
      ++this.generation;
      this.scheduleReconnect();
    };

    socket.onerror = () => {
      // Suppress — onclose handles reconnect; errors during StrictMode
      // double-fire are expected in dev and harmless.
    };
  }

  private invokeListener(type: string, listener: () => unknown) {
    const report = () => this.logger.warn("ws: listener failed", { type });
    try {
      const pending = listener();
      if (pending) void Promise.resolve(pending).catch(report);
    } catch {
      report();
    }
  }

  /**
   * Schedule a reconnection attempt with exponential backoff and jitter.
   * Retries indefinitely with a capped delay because the web/desktop UI
   * does not yet expose a visible disconnected state or manual retry action.
   */
  private scheduleReconnect() {
    const base = Math.min(
      RECONNECT_BASE_DELAY_MS * 2 ** this.reconnectAttempt,
      RECONNECT_MAX_DELAY_MS,
    );
    // ±20 % jitter so clients that disconnected at the same time don't
    // reconnect in lockstep.
    const jitter = base * 0.2 * (Math.random() * 2 - 1);
    const delay = Math.round(
      Math.min(base + jitter, RECONNECT_MAX_DELAY_MS),
    );

    this.reconnectAttempt++;
    this.logger.warn(
      `ws: disconnected, reconnecting in ${delay}ms (attempt ${this.reconnectAttempt})`,
    );
    const generation = this.generation;
    this.reconnectTimer = setTimeout(() => {
      if (this.generation === generation) this.connect();
    }, delay);
  }

  private onAuthenticated() {
    this.logger.info("connected");
    const recoveredConnection = this.hasConnectedBefore || this.reconnectAttempt > 0;
    this.reconnectAttempt = 0;
    if (recoveredConnection) {
      for (const cb of this.onReconnectCallbacks) {
        try {
          cb();
        } catch {
          // ignore reconnect callback errors
        }
      }
    }
    this.hasConnectedBefore = true;
  }

  disconnect() {
    ++this.generation;
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.ws) {
      // Remove handlers before close to prevent onclose from scheduling a reconnect
      this.ws.onopen = null;
      this.ws.onmessage = null;
      this.ws.onclose = null;
      this.ws.onerror = null;
      this.ws.close();
      this.ws = null;
    }
    this.hasConnectedBefore = false;
    this.reconnectAttempt = 0;
    this.handlers.clear();
    this.anyHandlers.clear();
    this.onReconnectCallbacks.clear();
  }

  on(event: WSEventType, handler: EventHandler) {
    if (!this.handlers.has(event)) {
      this.handlers.set(event, new Set());
    }
    this.handlers.get(event)!.add(handler);
    return () => {
      this.handlers.get(event)?.delete(handler);
    };
  }

  onAny(handler: (msg: WSMessage) => void) {
    this.anyHandlers.add(handler);
    return () => {
      this.anyHandlers.delete(handler);
    };
  }

  onReconnect(callback: () => void) {
    this.onReconnectCallbacks.add(callback);
    return () => {
      this.onReconnectCallbacks.delete(callback);
    };
  }

  send(message: WSMessage) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(message));
    }
  }
}
