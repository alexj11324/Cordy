import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  storedToken: null as string | null,
  api: {
    getMe: vi.fn(),
    setToken: vi.fn(),
    sendCode: vi.fn(),
    verifyCode: vi.fn(),
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
    clearLegacyGuestCredentials: vi.fn(async () => undefined),
  },
  workspaceStore: {
    restoreSlug: vi.fn(async () => null),
    clear: vi.fn(async () => undefined),
  },
  queryClient: { clear: vi.fn() },
}));

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
vi.mock("./query-client", () => ({ queryClient: mocks.queryClient }));
vi.mock("./workspace-store", () => ({
  useWorkspaceStore: { getState: () => mocks.workspaceStore },
}));

import { useAuthStore } from "./auth-store";

beforeEach(() => {
  mocks.storedToken = null;
  vi.clearAllMocks();
  mocks.secureStore.getToken.mockImplementation(async () => mocks.storedToken);
  mocks.secureStore.setToken.mockImplementation(async (token: string) => {
    mocks.storedToken = token;
  });
  mocks.secureStore.clearToken.mockImplementation(async () => {
    mocks.storedToken = null;
  });
  mocks.secureStore.clearLegacyGuestCredentials.mockResolvedValue(undefined);
  mocks.workspaceStore.restoreSlug.mockResolvedValue(null);
  mocks.workspaceStore.clear.mockResolvedValue(undefined);
  useAuthStore.setState({ user: null, isLoading: false });
});

describe("mobile auth state", () => {
  it("clears stale workspace and legacy Guest credentials without a token", async () => {
    await useAuthStore.getState().initialize();

    expect(mocks.secureStore.clearLegacyGuestCredentials).toHaveBeenCalledOnce();
    expect(mocks.workspaceStore.clear).toHaveBeenCalledOnce();
    expect(mocks.queryClient.clear).toHaveBeenCalledOnce();
  });

  it("refuses to restore a legacy Guest bearer", async () => {
    mocks.storedToken = `pbg_${"a".repeat(40)}`;

    await useAuthStore.getState().initialize();

    expect(mocks.api.getMe).not.toHaveBeenCalled();
    expect(mocks.api.setToken).toHaveBeenCalledWith(null);
    expect(mocks.secureStore.clearToken).toHaveBeenCalledOnce();
    expect(mocks.secureStore.clearLegacyGuestCredentials).toHaveBeenCalledOnce();
    expect(mocks.workspaceStore.clear).toHaveBeenCalledOnce();
    expect(useAuthStore.getState()).toMatchObject({ user: null, isLoading: false });
  });

  it("logs out remotely before clearing formal local credentials", async () => {
    mocks.storedToken = "formal-token";
    mocks.api.logout.mockResolvedValue(undefined);

    await useAuthStore.getState().logout();

    expect(mocks.api.logout).toHaveBeenCalledOnce();
    expect(mocks.secureStore.clearToken).toHaveBeenCalledOnce();
    expect(mocks.secureStore.clearLegacyGuestCredentials).toHaveBeenCalledOnce();
    expect(mocks.workspaceStore.clear).toHaveBeenCalledOnce();
    expect(mocks.api.setToken).toHaveBeenCalledWith(null);
  });

  it("still finishes local logout when cleanup fails", async () => {
    mocks.storedToken = "formal-token";
    mocks.api.logout.mockRejectedValue(new Error("offline"));
    mocks.secureStore.clearToken.mockRejectedValue(new Error("secure store"));
    mocks.workspaceStore.clear.mockRejectedValue(new Error("workspace store"));

    await useAuthStore.getState().logout();

    expect(mocks.api.setToken).toHaveBeenCalledWith(null);
    expect(mocks.queryClient.clear).toHaveBeenCalledOnce();
    expect(useAuthStore.getState().user).toBeNull();
  });
});
