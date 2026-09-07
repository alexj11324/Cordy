import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WSClient } from "./ws-client";
import type { WSEventType } from "@orvilo/core/types";

class MockWebSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;
  static instances: MockWebSocket[] = [];

  readyState = MockWebSocket.CONNECTING;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: (() => void) | null = null;
  readonly sent: string[] = [];

  constructor(readonly url: string) {
    MockWebSocket.instances.push(this);
  }

  open() {
    this.readyState = MockWebSocket.OPEN;
    this.onopen?.();
  }

  receive(frame: unknown) {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }

  send(frame: string) {
    this.sent.push(frame);
  }

  close() {
    this.readyState = MockWebSocket.CLOSED;
    this.onclose?.();
  }
}

function connectAuthenticatedClient() {
  const client = new WSClient({
    url: "wss://example.test/ws",
    token: "token",
    workspaceSlug: "workspace",
  });
  client.connect();
  const socket = MockWebSocket.instances[0];
  socket.open();
  socket.receive({ type: "auth_ack" });
  return { client, socket };
}

const issue = {
  id: "i", workspace_id: "workspace", number: 1, identifier: "WS-1", title: "Issue",
  description: null, status: "todo", priority: "none", creator_type: "member", creator_id: "user",
  parent_issue_id: null, project_id: null, position: 0, start_date: null, due_date: null,
  metadata: {}, created_at: "2026-01-01", updated_at: "2026-01-01",
};

describe("WSClient application heartbeat", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal("WebSocket", MockWebSocket);
    MockWebSocket.instances = [];
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("reconnects a stale OPEN socket through the jittered backoff path", () => {
    vi.spyOn(Math, "random").mockReturnValue(0);
    const { client, socket } = connectAuthenticatedClient();

    expect(socket.sent.map((frame) => JSON.parse(frame))).toEqual([
      { type: "auth", payload: { token: "token" } },
      { type: "ping" },
    ]);

    // No pong arrives even though the JS-visible readyState remains OPEN.
    vi.advanceTimersByTime(10_000);
    vi.advanceTimersByTime(1);

    expect(MockWebSocket.instances).toHaveLength(2);
    client.disconnect();
  });

  it("keeps a healthy socket connected when its pong arrives", () => {
    const { client, socket } = connectAuthenticatedClient();
    socket.receive({ type: "pong" });

    vi.advanceTimersByTime(10_000);

    expect(MockWebSocket.instances).toHaveLength(1);
    client.disconnect();
  });

  it("drops malformed and unknown frames before business listeners", () => {
    const { client, socket } = connectAuthenticatedClient();
    const handler = vi.fn();
    client.onAny(handler);
    for (const frame of [null, [], 12, { type: 12 }, { type: "issue:updated", payload: { issue: null } }, { type: "future:event", payload: {} }]) {
      expect(() => socket.receive(frame)).not.toThrow();
    }
    expect(handler).not.toHaveBeenCalled();
    socket.receive({ type: "issue:deleted", payload: { issue_id: "i", future_field: true } });
    expect(handler).toHaveBeenCalledWith({ type: "issue:deleted", payload: { issue_id: "i", future_field: true } });
    client.disconnect();
  });

  it("shares nested issue validation and preserves omitted versus false change flags", () => {
    const { client, socket } = connectAuthenticatedClient();
    const handler = vi.fn();
    client.on("issue:updated", handler);
    socket.receive({ type: "issue:updated", payload: { issue: { ...issue, labels: [null] } } });
    socket.receive({ type: "issue:updated", payload: { issue: { ...issue, reactions: [null] } } });
    socket.receive({ type: "issue:updated", payload: { issue, owner_changed: "false" } });
    expect(handler).not.toHaveBeenCalled();
    socket.receive({ type: "issue:updated", payload: { issue } });
    expect(handler.mock.calls[0][0]).not.toHaveProperty("owner_changed");
    expect(handler.mock.calls[0][0]).not.toHaveProperty("executor_changed");
    socket.receive({ type: "issue:updated", payload: { issue, owner_changed: false, executor_changed: false } });
    expect(handler.mock.calls[1][0]).toMatchObject({ owner_changed: false, executor_changed: false });
    client.disconnect();
  });

  it("dispatches current channel producer frames through the shared validator", () => {
    const { client, socket } = connectAuthenticatedClient();
    const created = vi.fn();
    const received = vi.fn();
    client.on("channel:created" as WSEventType, created);
    client.on("channel:message" as WSEventType, received);
    const channel = {
      id: "channel-1", workspace_id: "workspace-1", name: "Updates", slug: "updates",
      description: "", created_by: "user-1", archived_at: null,
      created_at: "2026-09-02T00:00:00Z", updated_at: "2026-09-02T00:00:00Z",
    };
    const message = {
      id: "message-1", workspace_id: "workspace-1", channel_id: channel.id,
      author_type: "member", author_id: "user-1", content: "Ready for review",
      parent_id: null, quoted_message_id: null,
      created_at: "2026-09-02T00:01:00Z", updated_at: "2026-09-02T00:01:00Z",
    };
    socket.receive({ type: "channel:created", payload: { channel: null } });
    socket.receive({ type: "channel:message", payload: { channel_id: channel.id, message: { ...message, content: {} } } });
    expect(created).not.toHaveBeenCalled();
    expect(received).not.toHaveBeenCalled();
    socket.receive({ type: "channel:created", payload: { channel } });
    socket.receive({ type: "channel:message", payload: { channel_id: channel.id, message } });
    expect(created).toHaveBeenCalledWith({ channel }, undefined);
    expect(received).toHaveBeenCalledWith({ channel_id: channel.id, message }, undefined);
    client.disconnect();
  });

  it("drops a numerically overflowing JSON frame and dispatches the next valid frame", () => {
    const { client, socket } = connectAuthenticatedClient();
    const handler = vi.fn();
    client.onAny(handler);
    expect(() => socket.onmessage?.({ data: '{"type":"issue_properties:changed","payload":{"issue_id":"i","properties":{"p":1e400}}}' })).not.toThrow();
    expect(handler).not.toHaveBeenCalled();
    socket.receive({ type: "issue:deleted", payload: { issue_id: "i" } });
    expect(handler).toHaveBeenCalledOnce();
    client.disconnect();
  });

  it("isolates throwing event and catch-all listeners without logging private errors", () => {
    const logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn() };
    const client = new WSClient({ url: "wss://example.test/ws", token: "token", workspaceSlug: "workspace", logger });
    const fail = () => { throw new Error("private listener content"); };
    const event = vi.fn();
    const any = vi.fn();
    client.on("issue:deleted", fail);
    client.on("issue:deleted", event);
    client.onAny(fail);
    client.onAny(any);
    client.connect();
    const socket = MockWebSocket.instances[0];
    expect(() => socket.receive({ type: "issue:deleted", payload: { issue_id: "i" } })).not.toThrow();
    expect(event).toHaveBeenCalledOnce();
    expect(any).toHaveBeenCalledOnce();
    expect(logger.warn).toHaveBeenCalledTimes(2);
    expect(JSON.stringify(logger.warn.mock.calls)).not.toContain("private listener content");
    client.disconnect();
  });

  it("observes rejecting async listeners and still delivers the same frame to later listeners", async () => {
    const logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn() };
    const client = new WSClient({ url: "wss://example.test/ws", token: "token", workspaceSlug: "workspace", logger });
    const observed = vi.fn();
    const reject = () => ({ then: (_resolve: unknown, onReject: (error: unknown) => void) => { observed(); onReject(new Error("private async content")); } });
    const later = vi.fn();
    client.on("issue:deleted", reject);
    client.onAny(reject);
    client.onAny(later);
    client.connect();
    MockWebSocket.instances[0].receive({ type: "issue:deleted", payload: { issue_id: "i" } });
    await Promise.resolve();
    await Promise.resolve();
    expect(observed).toHaveBeenCalledTimes(2);
    expect(later).toHaveBeenCalledOnce();
    expect(logger.warn).toHaveBeenCalledTimes(2);
    expect(JSON.stringify(logger.warn.mock.calls)).not.toContain("private async content");
    client.disconnect();
  });
});
