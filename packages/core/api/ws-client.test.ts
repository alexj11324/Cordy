// @vitest-environment node
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WSClient } from "./ws-client";
import type { WSMessage } from "../types/events";

// Capture URL passed to WebSocket so we can assert the connect-time
// query string.  We don't simulate the full WS lifecycle here — only the
// upgrade URL construction, which is what carries client identity.
class FakeWebSocket {
  static lastUrl: string | null = null;
  static lastInstance: FakeWebSocket | null = null;
  // Fields read by WSClient.connect()/disconnect(), all no-op here.
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  readyState = 0;
  constructor(url: string) {
    FakeWebSocket.lastUrl = url;
    FakeWebSocket.lastInstance = this;
  }
  close() {}
  send = vi.fn();
}

describe("WSClient", () => {
  beforeEach(() => {
    FakeWebSocket.lastUrl = null;
    FakeWebSocket.lastInstance = null;
    vi.stubGlobal("WebSocket", FakeWebSocket as unknown as typeof WebSocket);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("includes client identity in the upgrade URL when configured", () => {
    const ws = new WSClient("ws://example.test/ws", {
      identity: { platform: "desktop", version: "1.2.3", os: "macos" },
    });
    ws.setAuth("tok", "acme");
    ws.connect();

    const url = new URL(FakeWebSocket.lastUrl!);
    expect(url.searchParams.get("workspace_slug")).toBe("acme");
    expect(url.searchParams.get("client_platform")).toBe("desktop");
    expect(url.searchParams.get("client_version")).toBe("1.2.3");
    expect(url.searchParams.get("client_os")).toBe("macos");
    // Token must never appear in the URL — it is delivered as the first
    // WS message in token mode.
    expect(url.searchParams.has("token")).toBe(false);
  });

  it("omits client_* params when identity is not configured", () => {
    const ws = new WSClient("ws://example.test/ws");
    ws.setAuth("tok", "acme");
    ws.connect();

    const url = new URL(FakeWebSocket.lastUrl!);
    expect(url.searchParams.has("client_platform")).toBe(false);
    expect(url.searchParams.has("client_version")).toBe(false);
    expect(url.searchParams.has("client_os")).toBe(false);
  });

  it("isolates throwing event and catch-all listeners without logging private errors", () => {
    const logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };
    const ws = new WSClient("ws://example.test/ws", { logger });
    const event = vi.fn();
    const any = vi.fn();
    const fail = () => { throw new Error("private listener content"); };
    ws.on("issue:deleted", fail);
    ws.on("issue:deleted", event);
    ws.onAny(fail);
    ws.onAny(any);
    ws.connect();
    expect(() => FakeWebSocket.lastInstance!.onmessage?.({ data: JSON.stringify({ type: "issue:deleted", payload: { issue_id: "i" } }) })).not.toThrow();
    expect(event).toHaveBeenCalledOnce();
    expect(any).toHaveBeenCalledOnce();
    expect(logger.warn).toHaveBeenCalledTimes(2);
    expect(JSON.stringify(logger.warn.mock.calls)).not.toContain("private listener content");
    ws.disconnect();
  });

  it("observes rejecting async event and catch-all listeners and continues dispatching", async () => {
    const logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };
    const ws = new WSClient("ws://example.test/ws", { logger });
    const failure = new Error("private async content");
    const observed = vi.fn();
    const reject = () => ({ then: (_resolve: unknown, onReject: (error: unknown) => void) => { observed(); onReject(failure); } });
    const later = vi.fn();
    ws.on("issue:deleted", reject);
    ws.onAny(reject);
    ws.onAny(later);
    ws.connect();
    FakeWebSocket.lastInstance!.onmessage?.({ data: JSON.stringify({ type: "issue:deleted", payload: { issue_id: "i" } }) });
    await Promise.resolve();
    await Promise.resolve();
    expect(observed).toHaveBeenCalledTimes(2);
    expect(later).toHaveBeenCalledOnce();
    expect(logger.warn).toHaveBeenCalledTimes(2);
    expect(JSON.stringify(logger.warn.mock.calls)).not.toContain("private async content");
    ws.disconnect();
  });

  it("only includes the identity fields that are set", () => {
    const ws = new WSClient("ws://example.test/ws", {
      identity: { platform: "cli" },
    });
    ws.setAuth("tok", "acme");
    ws.connect();

    const url = new URL(FakeWebSocket.lastUrl!);
    expect(url.searchParams.get("client_platform")).toBe("cli");
    expect(url.searchParams.has("client_version")).toBe(false);
    expect(url.searchParams.has("client_os")).toBe(false);
  });

  it("truncates the logged payload when an unparseable frame is large", () => {
    const logger = {
      debug: vi.fn(),
      info: vi.fn(),
      warn: vi.fn(),
      error: vi.fn(),
    };
    const ws = new WSClient("ws://example.test/ws", { logger });
    ws.connect();

    const huge = "x".repeat(5000);
    FakeWebSocket.lastInstance!.onmessage?.({ data: huge });

    expect(logger.warn).toHaveBeenCalledTimes(1);
    const [, summary] = logger.warn.mock.calls[0] as [string, string];
    expect(summary.length).toBeLessThan(huge.length);
    expect(summary).toContain("truncated");
    expect(summary).toContain("5000");
    expect(summary.startsWith("x".repeat(200))).toBe(true);
  });

  it("logs and skips malformed frames without breaking later messages", () => {
    const logger = {
      debug: vi.fn(),
      info: vi.fn(),
      warn: vi.fn(),
      error: vi.fn(),
    };
    const ws = new WSClient("ws://example.test/ws", { logger });
    const handler = vi.fn();
    ws.on("issue:deleted", handler);
    ws.connect();

    expect(() => {
      FakeWebSocket.lastInstance!.onmessage?.({ data: `{"type":"issue` });
    }).not.toThrow();

    FakeWebSocket.lastInstance!.onmessage?.({
      data: JSON.stringify({
        type: "issue:deleted",
        payload: { issue_id: "issue-1" },
      }),
    });

    expect(logger.warn).toHaveBeenCalledWith(
      "ws: received unparseable message",
      `{"type":"issue`,
    );
    expect(handler).toHaveBeenCalledWith(
      { issue_id: "issue-1" },
      undefined,
      undefined,
    );
  });

  it("drops frames without a string type without throwing, and keeps dispatching", () => {
    // Regression for MUL-3418: a frame whose parsed JSON lacks a string `type`
    // (an out-of-protocol frame, or a bare JSON primitive) used to throw an
    // uncaught TypeError out of onmessage via `msg.type.split(...)` in a
    // downstream onAny handler, flooding `$exception` telemetry.
    const logger = {
      debug: vi.fn(),
      info: vi.fn(),
      warn: vi.fn(),
      error: vi.fn(),
    };
    const ws = new WSClient("ws://example.test/ws", { logger });

    // A downstream consumer that assumes a string type, exactly like the
    // realtime sync's onAny dispatcher.
    const anyHandler = vi.fn((msg: WSMessage) => msg.type.split(":")[0]);
    ws.onAny(anyHandler);
    const issueHandler = vi.fn();
    ws.on("issue:deleted", issueHandler);
    ws.connect();

    const badFrames = [
      JSON.stringify({ payload: {} }), // object, no type
      "42", // bare number
      "true", // bare bool
      "[]", // array
    ];
    for (const data of badFrames) {
      expect(() => {
        FakeWebSocket.lastInstance!.onmessage?.({ data });
      }).not.toThrow();
    }

    // Bad frames never reached any handler.
    expect(anyHandler).not.toHaveBeenCalled();
    expect(issueHandler).not.toHaveBeenCalled();

    // A valid frame after the bad ones still dispatches normally.
    FakeWebSocket.lastInstance!.onmessage?.({
      data: JSON.stringify({ type: "issue:deleted", payload: { issue_id: "i-1" } }),
    });
    expect(issueHandler).toHaveBeenCalledWith({ issue_id: "i-1" }, undefined, undefined);
    expect(anyHandler).toHaveBeenCalledTimes(1);

    // The drop is logged at most once per connection despite four bad frames.
    expect(logger.warn).toHaveBeenCalledTimes(1);
    expect(logger.warn.mock.calls[0]?.[0]).toBe(
      "ws: dropping invalid frame",
    );
  });

  it("passes actor_id and actor_type to event handlers", () => {
    const ws = new WSClient("ws://example.test/ws");
    ws.setAuth("tok", "acme");
    ws.connect();

    const handler = vi.fn();
    ws.on("issue:deleted", handler);

    const fakeWs = (ws as any).ws as FakeWebSocket;
    fakeWs.onmessage?.({
      data: JSON.stringify({
        type: "issue:deleted",
        payload: { issue_id: "issue-1" },
        actor_id: "user-123",
        actor_type: "user",
      }),
    });

    expect(handler).toHaveBeenCalledWith(
      { issue_id: "issue-1" },
      "user-123",
      "user",
    );
  });

  it("drops malformed known payloads and unknown events before every subscription", () => {
    const logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };
    const ws = new WSClient("ws://example.test/ws", { logger });
    const handler = vi.fn();
    const any = vi.fn();
    ws.on("task:message", handler);
    ws.onAny(any);
    ws.connect();
    for (const payload of [null, {}, { task_id: "t", seq: "1", type: "text" }]) {
      FakeWebSocket.lastInstance!.onmessage?.({ data: JSON.stringify({ type: "task:message", payload }) });
    }
    FakeWebSocket.lastInstance!.onmessage?.({ data: JSON.stringify({ type: "task:future_event", payload: {} }) });
    expect(handler).not.toHaveBeenCalled();
    expect(any).not.toHaveBeenCalled();
    expect(logger.warn).toHaveBeenCalledTimes(1);
    FakeWebSocket.lastInstance!.onmessage?.({ data: JSON.stringify({ type: "task:message", payload: { task_id: "t", seq: 1, type: "text" } }) });
    expect(handler).toHaveBeenCalledTimes(1);
    expect(any).toHaveBeenCalledTimes(1);
    ws.disconnect();
  });

  it("ignores callbacks retained by a socket replaced during a workspace switch", () => {
    vi.useFakeTimers();
    try {
      const ws = new WSClient("ws://example.test/ws");
      ws.setAuth("token-a", "workspace-a");
      ws.connect();
      const old = FakeWebSocket.lastInstance!;
      const oldOpen = old.onopen!;
      const oldMessage = old.onmessage!;
      const oldClose = old.onclose!;
      ws.disconnect();
      ws.setAuth("token-b", "workspace-b");
      const handler = vi.fn();
      const reconnect = vi.fn();
      ws.on("issue:deleted", handler);
      ws.onReconnect(reconnect);
      ws.connect();
      const current = FakeWebSocket.lastInstance!;
      oldOpen();
      oldMessage({ data: JSON.stringify({ type: "auth_ack" }) });
      oldMessage({ data: JSON.stringify({ type: "issue:deleted", payload: { issue_id: "a" } }) });
      oldClose();
      expect(current.send).not.toHaveBeenCalled();
      expect(handler).not.toHaveBeenCalled();
      expect(reconnect).not.toHaveBeenCalled();
      expect(vi.getTimerCount()).toBe(0);
      current.onopen?.();
      expect(current.send).toHaveBeenCalledWith(JSON.stringify({ type: "auth", payload: { token: "token-b" } }));
      current.onmessage?.({ data: JSON.stringify({ type: "issue:deleted", payload: { issue_id: "b" } }) });
      expect(handler).toHaveBeenCalledWith({ issue_id: "b" }, undefined, undefined);
      ws.disconnect();
    } finally { vi.useRealTimers(); }
  });

  it("delivers authenticated reconnect once and ignores a closed socket's late events", () => {
    vi.useFakeTimers();
    try {
      const ws = new WSClient("ws://example.test/ws");
      const reconnect = vi.fn();
      const any = vi.fn();
      ws.onReconnect(reconnect);
      ws.onAny(any);
      ws.connect();
      const first = FakeWebSocket.lastInstance!;
      first.onmessage?.({ data: '{"type":"auth_ack"}' });
      first.onclose?.();
      first.onmessage?.({ data: '{"type":"issue:deleted","payload":{"issue_id":"old"}}' });
      first.onclose?.();
      expect(vi.getTimerCount()).toBe(1);
      vi.runOnlyPendingTimers();
      const second = FakeWebSocket.lastInstance!;
      second.onmessage?.({ data: '{"type":"auth_ack"}' });
      second.onmessage?.({ data: '{"type":"auth_ack"}' });
      expect(reconnect).toHaveBeenCalledTimes(1);
      expect(any).not.toHaveBeenCalled();
      ws.disconnect();
    } finally { vi.useRealTimers(); }
  });

  // ── Reconnect backoff tests ────────────────────────────────────────

  describe("reconnect backoff", () => {
    let setTimeoutSpy: ReturnType<typeof vi.spyOn>;

    beforeEach(() => {
      vi.useFakeTimers();
      setTimeoutSpy = vi.spyOn(globalThis, "setTimeout");
    });

    afterEach(() => {
      setTimeoutSpy.mockRestore();
      vi.useRealTimers();
    });

    /** Return the delay (ms) passed to the most recent setTimeout call. */
    function lastTimerDelay(): number {
      const calls = setTimeoutSpy.mock.calls;
      return calls[calls.length - 1]?.[1] as number;
    }

    function simulateDisconnect() {
      FakeWebSocket.lastInstance!.onclose?.();
    }

    /** Simulate the server acknowledging the auth token. In token mode the
     *  client sends `{type:"auth"}` on open, and the server replies with
     *  `{type:"auth_ack"}`. Only then does `onAuthenticated()` fire and
     *  reset the reconnect counter. */
    function simulateAuthAck() {
      FakeWebSocket.lastInstance!.onmessage?.({
        data: JSON.stringify({ type: "auth_ack" }),
      });
    }

    it("uses ~1000ms base delay for the first reconnect attempt", () => {
      // Pin Math.random so jitter is deterministic: random()=0.5 → jitter=0.
      vi.stubGlobal(
        "Math",
        new Proxy(Math, {
          get(target, prop) {
            if (prop === "random") return () => 0.5;
            return (target as any)[prop];
          },
        }),
      );

      const ws = new WSClient("ws://example.test/ws");
      ws.connect();
      simulateDisconnect();

      expect(lastTimerDelay()).toBe(1000);
    });

    it("doubles the base delay on consecutive failures (exponential)", () => {
      vi.stubGlobal(
        "Math",
        new Proxy(Math, {
          get(target, prop) {
            if (prop === "random") return () => 0.5;
            return (target as any)[prop];
          },
        }),
      );

      const ws = new WSClient("ws://example.test/ws");
      ws.connect();

      // Attempt 1: base = 1000 * 2^0 = 1000
      simulateDisconnect();
      expect(lastTimerDelay()).toBe(1000);

      // Fire the timer → connect() → new FakeWebSocket → simulate disconnect
      vi.advanceTimersByTime(1000);
      // Attempt 2: base = 1000 * 2^1 = 2000
      simulateDisconnect();
      expect(lastTimerDelay()).toBe(2000);

      vi.advanceTimersByTime(2000);
      // Attempt 3: base = 1000 * 2^2 = 4000
      simulateDisconnect();
      expect(lastTimerDelay()).toBe(4000);

      vi.advanceTimersByTime(4000);
      // Attempt 4: base = 1000 * 2^3 = 8000
      simulateDisconnect();
      expect(lastTimerDelay()).toBe(8000);
    });

    it("caps the delay at 30s even after many failures", () => {
      vi.stubGlobal(
        "Math",
        new Proxy(Math, {
          get(target, prop) {
            if (prop === "random") return () => 0.5;
            return (target as any)[prop];
          },
        }),
      );

      const ws = new WSClient("ws://example.test/ws");
      ws.connect();

      // Drive through enough failures to exceed the cap:
      // 2^5 = 32000 > 30000, so attempt 6 should be capped.
      const delays = [1000, 2000, 4000, 8000, 16000];
      for (const d of delays) {
        simulateDisconnect();
        expect(lastTimerDelay()).toBe(d);
        vi.advanceTimersByTime(d);
      }

      // Attempt 6: 1000 * 2^5 = 32000 → capped to 30000
      simulateDisconnect();
      expect(lastTimerDelay()).toBe(30000);
    });

    it("applies jitter so delays vary with Math.random", () => {
      // Stub Math.random to alternate between 0 (jitter = -20%) and
      // 1 (jitter = +20%), producing deterministic min/max delays.
      let callCount = 0;
      vi.stubGlobal(
        "Math",
        new Proxy(Math, {
          get(target, prop) {
            if (prop === "random") return () => (callCount++ % 2 === 0 ? 0 : 1);
            return (target as any)[prop];
          },
        }),
      );

      const ws = new WSClient("ws://example.test/ws");

      // First disconnect: random()=0 → jitter = 1000 * 0.2 * (0*2-1) = -200 → 800ms
      ws.connect();
      simulateDisconnect();
      const delay1 = lastTimerDelay();

      // Second disconnect: random()=1 → jitter = 1000 * 0.2 * (1*2-1) = +200 → 1200ms
      vi.clearAllTimers();
      ws.disconnect();
      ws.connect();
      simulateDisconnect();
      const delay2 = lastTimerDelay();

      // The two delays must differ and fall within [800, 1200].
      expect(delay1).toBe(800);
      expect(delay2).toBe(1200);
    });

    it("resets the attempt counter on successful authentication", () => {
      vi.stubGlobal(
        "Math",
        new Proxy(Math, {
          get(target, prop) {
            if (prop === "random") return () => 0.5;
            return (target as any)[prop];
          },
        }),
      );

      const ws = new WSClient("ws://example.test/ws");
      ws.setAuth("tok", "acme");
      ws.connect();

      // Two failures: delays 1000, 2000
      simulateDisconnect();
      expect(lastTimerDelay()).toBe(1000);
      vi.advanceTimersByTime(1000);

      simulateDisconnect();
      expect(lastTimerDelay()).toBe(2000);
      vi.advanceTimersByTime(2000);

      // Successful connection resets the counter.
      simulateAuthAck();

      // Next failure should be back to the base delay.
      simulateDisconnect();
      expect(lastTimerDelay()).toBe(1000);
    });

    it("notifies reconnect when the initial connection succeeds only after a retry", () => {
      const ws = new WSClient("ws://example.test/ws");
      ws.setAuth("tok", "acme");
      const onReconnect = vi.fn();
      ws.onReconnect(onReconnect);
      ws.connect();

      simulateDisconnect();
      vi.runOnlyPendingTimers();
      simulateAuthAck();

      expect(onReconnect).toHaveBeenCalledTimes(1);
    });

    it("keeps retrying indefinitely with capped delay", () => {
      vi.stubGlobal(
        "Math",
        new Proxy(Math, {
          get(target, prop) {
            if (prop === "random") return () => 0.5;
            return (target as any)[prop];
          },
        }),
      );

      const logger = {
        debug: vi.fn(),
        info: vi.fn(),
        warn: vi.fn(),
        error: vi.fn(),
      };
      const ws = new WSClient("ws://example.test/ws", { logger });
      ws.connect();

      // Drive past the old 20-attempt limit — all should schedule a reconnect.
      for (let i = 0; i < 25; i++) {
        simulateDisconnect();
        const delay = lastTimerDelay();
        // Once past attempt 5 (base > 30000), delay should be capped at 30s.
        if (i >= 5) {
          expect(delay).toBe(30000);
        }
        vi.advanceTimersByTime(delay);
      }

      // The 26th disconnect should STILL schedule a reconnect (no give-up).
      const timerCountBefore = setTimeoutSpy.mock.calls.length;
      simulateDisconnect();
      expect(setTimeoutSpy.mock.calls.length).toBe(timerCountBefore + 1);
      expect(lastTimerDelay()).toBe(30000);
      // No "giving up" error should have been logged.
      expect(logger.error).not.toHaveBeenCalled();
    });

    it("disconnect() cancels a pending reconnect and resets the counter", () => {
      vi.stubGlobal(
        "Math",
        new Proxy(Math, {
          get(target, prop) {
            if (prop === "random") return () => 0.5;
            return (target as any)[prop];
          },
        }),
      );

      const ws = new WSClient("ws://example.test/ws");
      ws.connect();

      // Two failures to bump the counter.
      simulateDisconnect();
      vi.advanceTimersByTime(1000);
      simulateDisconnect();
      // A reconnect timer is now pending.

      // Explicit disconnect should cancel the pending timer.
      ws.disconnect();
      vi.advanceTimersByTime(10_000);
      // No new WebSocket should have been created after the disconnect.
      // The last URL should still be from the second connect() call.

      // A fresh connect() after disconnect should start from attempt 0.
      ws.connect();
      simulateDisconnect();
      expect(lastTimerDelay()).toBe(1000);
    });
  });
});
