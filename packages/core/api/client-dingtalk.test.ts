// @vitest-environment node

import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiClient } from "./client";

const route = {
  id: "route-1", workspace_id: "workspace-1", installation_id: "bot-1",
  conversation_id: "cid-team", conversation_title: "Release team", agent_id: "agent-1",
  discovered_at: "2026-09-03T12:00:00Z", updated_at: "2026-09-03T12:00:00Z",
};

function mockResponse(body: unknown, status = 200) {
  const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify(body), {
    status, headers: { "Content-Type": "application/json" },
  }));
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

afterEach(() => vi.unstubAllGlobals());

describe("DingTalk group-route API", () => {
  const client = new ApiClient("https://api.example.test");

  it("reads the workspace route inventory through the shared client", async () => {
    const fetchMock = mockResponse({ routes: [{ ...route, future_field: "kept" }] });
    expect(await client.listDingTalkGroupRoutes("workspace-1")).toEqual({
      routes: [{ ...route, future_field: "kept" }],
    });
    expect(fetchMock.mock.calls[0]?.[0]).toBe("https://api.example.test/api/workspaces/workspace-1/dingtalk/group-routes");
  });

  it("degrades a malformed inventory without producing editable fake rows", async () => {
    mockResponse({ routes: [{ ...route, id: 42 }] });
    expect(await client.listDingTalkGroupRoutes("workspace-1")).toEqual({ routes: [] });
  });

  it("patches the selected route with the exact agent id", async () => {
    const fetchMock = mockResponse({ ...route, agent_id: "agent-2" });
    expect(await client.updateDingTalkGroupRoute("workspace-1", "route-1", { agent_id: "agent-2" }))
      .toMatchObject({ id: "route-1", agent_id: "agent-2" });
    expect(fetchMock.mock.calls[0]?.[0]).toBe("https://api.example.test/api/workspaces/workspace-1/dingtalk/group-routes/route-1");
    expect(fetchMock.mock.calls[0]?.[1]).toMatchObject({ method: "PATCH", body: JSON.stringify({ agent_id: "agent-2" }) });
  });

  it("preserves permission errors instead of treating them as an empty inventory", async () => {
    mockResponse({ error: "workspace access denied" }, 403);
    await expect(client.listDingTalkGroupRoutes("workspace-1")).rejects.toMatchObject({ status: 403 });
  });

  it("returns an identifiable empty fallback for a malformed update", async () => {
    mockResponse({ ...route, agent_id: false });
    expect(await client.updateDingTalkGroupRoute("workspace-1", "route-1", { agent_id: "agent-2" }))
      .toMatchObject({ id: "", agent_id: "" });
  });

  it("preserves the routing capability and treats malformed flags as unsupported", async () => {
    mockResponse({ configured: true, installations: [], group_routing_supported: true });
    expect(await client.listDingTalkInstallations("workspace-1")).toMatchObject({ group_routing_supported: true });
    mockResponse({ configured: true, installations: [], group_routing_supported: "true" });
    expect(await client.listDingTalkInstallations("workspace-1")).not.toMatchObject({ group_routing_supported: "true" });
  });
});
