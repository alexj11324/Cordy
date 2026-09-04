import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiClient } from "../api/client";

afterEach(() => {
  vi.unstubAllGlobals();
});

function jsonResponse(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

describe("ApiClient Weixin device flow", () => {
  it("uses workspace-scoped list and begin endpoints", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        jsonResponse({ installations: [], configured: true, install_supported: true }),
      )
      .mockResolvedValueOnce(
        jsonResponse({
          session_id: "session-1",
          qr_code_url: "https://example.test/qr",
          expires_in_seconds: 600,
          poll_interval_seconds: 3,
        }),
      );
    vi.stubGlobal("fetch", fetchMock);
    const client = new ApiClient("https://api.example.test");

    await expect(client.listWeixinInstallations("workspace/1")).resolves.toMatchObject({
      configured: true,
      install_supported: true,
    });
    await expect(client.beginWeixinInstall("workspace/1", "agent/1")).resolves.toEqual({
      session_id: "session-1",
      qr_code_url: "https://example.test/qr",
      expires_in_seconds: 600,
      poll_interval_seconds: 3,
    });

    expect(fetchMock.mock.calls[0]?.[0]).toBe(
      "https://api.example.test/api/workspaces/workspace%2F1/weixin/installations",
    );
    expect(fetchMock.mock.calls[1]?.[0]).toBe(
      "https://api.example.test/api/workspaces/workspace%2F1/weixin/install/begin?agent_id=agent%2F1",
    );
    expect(fetchMock.mock.calls[1]?.[1]).toMatchObject({ method: "POST" });
  });

  it("omits agent_id when beginning a workspace Hub installation", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      jsonResponse({
        session_id: "session-hub",
        qr_code_url: "https://example.test/qr",
        expires_in_seconds: 600,
        poll_interval_seconds: 3,
      }),
    );
    vi.stubGlobal("fetch", fetchMock);
    const client = new ApiClient("https://api.example.test");

    await client.beginWeixinInstall("workspace/1", undefined);

    expect(fetchMock.mock.calls[0]?.[0]).toBe(
      "https://api.example.test/api/workspaces/workspace%2F1/weixin/install/begin",
    );
  });

  it("sends a verification code only for the status retry and preserves redeem payloads", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse({ status: "need_verify_code" }))
      .mockResolvedValueOnce(jsonResponse({ status: "success", installation_id: "installation-1" }))
      .mockResolvedValueOnce(new Response(null, { status: 204 }))
      .mockResolvedValueOnce(
        jsonResponse({
          workspace_id: "workspace-1",
          installation_id: "installation-1",
          weixin_user_id: "wx-user-1",
        }),
      );
    vi.stubGlobal("fetch", fetchMock);
    const client = new ApiClient("https://api.example.test");

    await expect(client.getWeixinInstallStatus("workspace-1", "session-1")).resolves.toEqual({
      status: "need_verify_code",
    });
    await expect(
      client.getWeixinInstallStatus("workspace-1", "session-1", " 2468 "),
    ).resolves.toEqual({ status: "success", installation_id: "installation-1" });
    await client.deleteWeixinInstallation("workspace-1", "installation-1");
    await expect(client.redeemWeixinBindingToken("raw token")).resolves.toEqual({
      workspace_id: "workspace-1",
      installation_id: "installation-1",
      weixin_user_id: "wx-user-1",
    });

    expect(fetchMock.mock.calls[0]?.[0]).toBe(
      "https://api.example.test/api/workspaces/workspace-1/weixin/install/session-1/status",
    );
    expect(fetchMock.mock.calls[1]?.[0]).toBe(
      "https://api.example.test/api/workspaces/workspace-1/weixin/install/session-1/status?verify_code=2468",
    );
    expect(fetchMock.mock.calls[2]?.[0]).toBe(
      "https://api.example.test/api/workspaces/workspace-1/weixin/installations/installation-1",
    );
    expect(fetchMock.mock.calls[2]?.[1]).toMatchObject({ method: "DELETE" });
    expect(JSON.parse(String(fetchMock.mock.calls[3]?.[1]?.body))).toEqual({ token: "raw token" });
  });

  it("falls back to safe empty data for malformed successful responses", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(jsonResponse({ status: 42 })));
    await expect(
      new ApiClient("https://api.example.test").getWeixinInstallStatus("workspace-1", "session-1"),
    ).resolves.toEqual({ status: "pending" });
  });
});
