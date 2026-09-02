import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiClient } from "../api/client";

afterEach(() => vi.unstubAllGlobals());

describe("Agent thread API client", () => {
  it("sends the continuation receipt only in Idempotency-Key", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      continuation_task_id: "task-2",
      status: "queued",
    }), { status: 200, headers: { "Content-Type": "application/json" } }));
    vi.stubGlobal("fetch", fetchMock);

    const client = new ApiClient("https://api.example.test");
    await client.continueAgentThread("task-1", {
      content: "continue the task",
      idempotency_key: "receipt-1",
    });

    const [, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect((init.headers as Record<string, string>)["Idempotency-Key"]).toBe("receipt-1");
    expect(JSON.parse(String(init.body))).toEqual({ content: "continue the task" });
  });

  it("rejects a malformed continuation response", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(JSON.stringify({
      status: "queued",
    }), { status: 200, headers: { "Content-Type": "application/json" } })));
    await expect(new ApiClient("https://api.example.test").continueAgentThread("task-1", {
      content: "continue",
      idempotency_key: "receipt-2",
    })).rejects.toThrow("Invalid Agent thread continuation response");
  });
});
