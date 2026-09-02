import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => {
  const guestToken = `pbg_${"a".repeat(40)}`;
  return {
    guestToken,
    storedToken: null as string | null,
    guestCredentials: null as {
      token: string;
      sessionId: string | null;
    } | null,
    api: {
      createGuestAuth: vi.fn(),
      getMe: vi.fn(),
      setToken: vi.fn(),
      sendCode: vi.fn(),
      verifyCode: vi.fn(),
      claimGuestSession: vi.fn(),
      logout: vi.fn(),
    },
    secureStore: {
      getToken: vi.fn(async () => mocks.storedToken),
      setToken: vi.fn(async (token: string) => {
        mocks.storedToken = token;
      }),
      clearToken: vi.fn(async () => {
        mocks.storedToken = null;
      }),
    },
    guestStore: {
      saveGuestCredentials: vi.fn(
        async (token: string, sessionId?: string) => {
          mocks.guestCredentials = { token, sessionId: sessionId ?? null };
        },
      ),
      getGuestCredentials: vi.fn(async () => mocks.guestCredentials),
      clearGuestCredentials: vi.fn(async () => {
        mocks.guestCredentials = null;
      }),
    },
    workspaceStore: {
      restoreSlug: vi.fn(async () => null),
      clear: vi.fn(async () => undefined),
    },
    queryClient: {
      clear: vi.fn(),
    },
  };
});

vi.mock("./api", () => ({
  api: mocks.api,
  ApiError: class ApiError extends Error {
    readonly status: number;
    constructor(message: string, status: number) {
      super(message);
      this.status = status;
    }
  },
}));

vi.mock("./secure-storage", () => mocks.secureStore);
vi.mock("./guest-storage", () => mocks.guestStore);
vi.mock("./query-client", () => ({ queryClient: mocks.queryClient }));
vi.mock("./workspace-store", () => ({
  useWorkspaceStore: {
    getState: () => mocks.workspaceStore,
  },
}));

import { useAuthStore } from "./auth-store";

const user = {
  id: "018f03a0-c4d2-7a37-ae4d-5aa45de12f12",
  name: "Guest",
  email: "guest@example.invalid",
  avatar_url: null,
  onboarded_at: null,
  onboarding_questionnaire: {},
  starter_content_state: null,
  language: null,
  profile_description: "",
  timezone: null,
  created_at: "",
  updated_at: "",
};

const guestSession = {
  id: "018f03a0-c4d2-7a37-ae4d-5aa45de12f11",
  user_id: user.id,
  status: "claimed" as const,
  created_at: "2026-09-02T00:00:00Z",
  claimed_at: "2026-09-02T00:01:00Z",
  claimed_by: "018f03a0-c4d2-7a37-ae4d-5aa45de12f13",
};

beforeEach(() => {
  mocks.storedToken = null;
  mocks.guestCredentials = null;
  for (const group of [mocks.api, mocks.secureStore, mocks.guestStore]) {
    for (const value of Object.values(group)) {
      if (typeof value === "function" && "mockReset" in value) {
        value.mockReset();
      }
    }
  }
  for (const value of Object.values(mocks.workspaceStore)) {
    if (typeof value === "function" && "mockReset" in value) {
      value.mockReset();
    }
  }
  mocks.secureStore.getToken.mockImplementation(async () => mocks.storedToken);
  mocks.secureStore.setToken.mockImplementation(async (token: string) => {
    mocks.storedToken = token;
  });
  mocks.secureStore.clearToken.mockImplementation(async () => {
    mocks.storedToken = null;
  });
  mocks.guestStore.saveGuestCredentials.mockImplementation(
    async (token: string, sessionId?: string) => {
      mocks.guestCredentials = { token, sessionId: sessionId ?? null };
    },
  );
  mocks.guestStore.getGuestCredentials.mockImplementation(
    async () => mocks.guestCredentials,
  );
  mocks.guestStore.clearGuestCredentials.mockImplementation(async () => {
    mocks.guestCredentials = null;
  });
  mocks.workspaceStore.restoreSlug.mockResolvedValue(null);
  mocks.workspaceStore.clear.mockResolvedValue(undefined);
  mocks.queryClient.clear.mockReset();
  useAuthStore.setState({
    user: null,
    isGuest: false,
    isLoading: false,
  });
});

describe("mobile guest auth state", () => {
  it("creates a server-backed guest session and persists the opaque bearer", async () => {
    mocks.api.createGuestAuth.mockResolvedValue({
      token: mocks.guestToken,
      user,
      session_id: guestSession.id,
    });

    const result = await useAuthStore.getState().continueAsGuest();

    expect(result.id).toBe(user.id);
    expect(useAuthStore.getState().isGuest).toBe(true);
    expect(mocks.secureStore.setToken).toHaveBeenCalledWith(mocks.guestToken);
    expect(mocks.guestStore.saveGuestCredentials).toHaveBeenCalledWith(
      mocks.guestToken,
      guestSession.id,
    );
    expect(mocks.api.setToken).toHaveBeenCalledWith(mocks.guestToken);
  });

  it("recognizes a restored pbg bearer without trusting a client guest flag", async () => {
    mocks.storedToken = mocks.guestToken;
    mocks.api.getMe.mockResolvedValue(user);

    await useAuthStore.getState().initialize();

    expect(useAuthStore.getState().isGuest).toBe(true);
    expect(useAuthStore.getState().user).toEqual(user);
  });

  it("clears stale workspace state and cache when no bearer exists", async () => {
    await useAuthStore.getState().initialize();

    expect(mocks.workspaceStore.clear).toHaveBeenCalledOnce();
    expect(mocks.queryClient.clear).toHaveBeenCalledOnce();
  });

  it("claims a stored guest session only from a formal account", async () => {
    useAuthStore.setState({ user, isGuest: false });
    mocks.guestCredentials = {
      token: mocks.guestToken,
      sessionId: guestSession.id,
    };
    mocks.api.claimGuestSession.mockResolvedValue(guestSession);

    await expect(
      useAuthStore.getState().claimGuestSession(),
    ).resolves.toEqual(guestSession);

    expect(mocks.api.claimGuestSession).toHaveBeenCalledWith(
      guestSession.id,
      mocks.guestToken,
    );
    expect(mocks.guestStore.clearGuestCredentials).toHaveBeenCalledOnce();
  });

  it("does not let a guest bearer claim its own session", async () => {
    useAuthStore.setState({ user, isGuest: true });

    await expect(
      useAuthStore.getState().claimGuestSession(guestSession.id),
    ).rejects.toThrow("Formal login required");
    expect(mocks.api.claimGuestSession).not.toHaveBeenCalled();
  });

  it("calls server logout before clearing a guest bearer and workspace", async () => {
    mocks.storedToken = mocks.guestToken;
    useAuthStore.setState({ user, isGuest: true });
    mocks.api.logout.mockResolvedValue(undefined);

    await useAuthStore.getState().logout();

    expect(mocks.api.logout).toHaveBeenCalledOnce();
    expect(mocks.secureStore.clearToken).toHaveBeenCalledOnce();
    expect(mocks.guestStore.clearGuestCredentials).toHaveBeenCalledOnce();
    expect(mocks.workspaceStore.clear).toHaveBeenCalledOnce();
    expect(mocks.queryClient.clear).toHaveBeenCalledOnce();
    expect(mocks.api.setToken).toHaveBeenCalledWith(null);
    expect(useAuthStore.getState()).toMatchObject({
      user: null,
      isGuest: false,
    });
  });

  it("still clears local credentials when server logout is unavailable", async () => {
    mocks.storedToken = mocks.guestToken;
    mocks.api.logout.mockRejectedValue(new Error("offline"));

    await useAuthStore.getState().logout();

    expect(mocks.secureStore.clearToken).toHaveBeenCalledOnce();
    expect(mocks.guestStore.clearGuestCredentials).toHaveBeenCalledOnce();
    expect(mocks.workspaceStore.clear).toHaveBeenCalledOnce();
    expect(mocks.queryClient.clear).toHaveBeenCalledOnce();
    expect(mocks.api.setToken).toHaveBeenCalledWith(null);
    expect(useAuthStore.getState().user).toBeNull();
  });

  it("finishes local logout when secure cleanup fails", async () => {
    mocks.storedToken = mocks.guestToken;
    useAuthStore.setState({ user, isGuest: true });
    mocks.secureStore.clearToken.mockRejectedValue(new Error("secure store"));
    mocks.guestStore.clearGuestCredentials.mockRejectedValue(
      new Error("secure store"),
    );
    mocks.workspaceStore.clear.mockRejectedValue(new Error("workspace store"));

    await useAuthStore.getState().logout();

    expect(mocks.api.setToken).toHaveBeenCalledWith(null);
    expect(mocks.queryClient.clear).toHaveBeenCalledOnce();
    expect(useAuthStore.getState()).toMatchObject({
      user: null,
      isGuest: false,
    });
  });
});
