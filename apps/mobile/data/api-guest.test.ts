import { beforeEach, describe, expect, it, vi } from "vitest";

vi.hoisted(() => {
  process.env.EXPO_PUBLIC_API_URL = "https://api.example.test";
});

vi.mock("expo-secure-store", () => ({
  getItemAsync: vi.fn(async () => null),
  setItemAsync: vi.fn(async () => undefined),
  deleteItemAsync: vi.fn(async () => undefined),
}));

vi.mock("./workspace-store", () => ({
  getCurrentSlug: () => null,
  useWorkspaceStore: { getState: () => ({ currentWorkspaceSlug: null }) },
}));

import { api, ApiError } from "./api";

const token = `pbg_${"a".repeat(40)}`;
const sessionId = "018f03a0-c4d2-7a37-ae4d-5aa45de12f11";
const userId = "018f03a0-c4d2-7a37-ae4d-5aa45de12f12";

const guestUser = {
  id: userId,
  name: "Guest",
  email: "guest@example.invalid",
};

const session = {
  id: sessionId,
  user_id: userId,
  status: "active",
  created_at: "2026-09-02T00:00:00Z",
  claimed_at: null,
  claimed_by: null,
};

function jsonResponse(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

beforeEach(() => {
  vi.stubGlobal("fetch", vi.fn());
  api.setToken("formal-token");
});

describe("mobile guest API routes", () => {
  it("creates guest auth through the public endpoint", async () => {
    const fetchMock = vi.mocked(fetch);
    fetchMock.mockResolvedValueOnce(
      jsonResponse({ token, user: guestUser, session_id: sessionId }),
    );

    const result = await api.createGuestAuth();

    expect(result.token).toBe(token);
    expect(fetchMock).toHaveBeenCalledWith(
      "https://api.example.test/auth/guest",
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("binds claim and revoke to the session path and guest proof", async () => {
    const fetchMock = vi.mocked(fetch);
    fetchMock
      .mockResolvedValueOnce(jsonResponse({ ...session, status: "claimed" }))
      .mockResolvedValueOnce(jsonResponse({ ...session, status: "revoked" }));

    await api.claimGuestSession(sessionId, token);
    await api.revokeGuestSession(sessionId, token);

    expect(fetchMock.mock.calls[0]?.[0]).toBe(
      `https://api.example.test/api/guest-sessions/${sessionId}/claim`,
    );
    expect(fetchMock.mock.calls[1]?.[0]).toBe(
      `https://api.example.test/api/guest-sessions/${sessionId}/revoke`,
    );
    expect(fetchMock.mock.calls[0]?.[1]).toEqual(
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ token }),
      }),
    );
  });

  it("rejects malformed proof before it can reach the server", async () => {
    const fetchMock = vi.mocked(fetch);

    await expect(api.revokeGuestSession(sessionId, "jwt-token")).rejects.toThrow(
      "Invalid guest token",
    );
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("fails closed on a malformed lifecycle response", async () => {
    const fetchMock = vi.mocked(fetch);
    fetchMock.mockResolvedValueOnce(jsonResponse({ id: sessionId }));

    await expect(api.getGuestSession(sessionId)).rejects.toBeInstanceOf(ApiError);
  });

  it("uses the public logout endpoint so server middleware can revoke the bearer", async () => {
    const fetchMock = vi.mocked(fetch);
    fetchMock.mockResolvedValueOnce(jsonResponse({ message: "logged out" }));

    await api.logout();

    expect(fetchMock).toHaveBeenCalledWith(
      "https://api.example.test/auth/logout",
      expect.objectContaining({ method: "POST" }),
    );
  });
});
