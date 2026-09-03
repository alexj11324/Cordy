import { describe, expect, it, vi } from "vitest";
import type { ApiClient } from "../api/client";
import type { StorageAdapter, User } from "../types";
import { createAuthStore } from "./store";

const fakeUser: User = {
  id: "u1",
  name: "Alice",
  email: "alice@example.com",
  avatar_url: null,
} as User;

function makeStorage(initial: Record<string, string> = {}): StorageAdapter & {
  snapshot: () => Record<string, string>;
} {
  const data = { ...initial };
  return {
    getItem: (k) => data[k] ?? null,
    setItem: (k, v) => {
      data[k] = v;
    },
    removeItem: (k) => {
      delete data[k];
    },
    snapshot: () => ({ ...data }),
  };
}

function makeApi(): ApiClient {
  return {
    setToken: vi.fn(),
  } as unknown as ApiClient;
}

describe("authStore", () => {
  it("publishes a retry request instead of silently ignoring it", () => {
    const storage = makeStorage({ patchbay_token: "t" });
    const api = makeApi();
    const store = createAuthStore({ api, storage });

    store.setState({ isLoading: true, status: "recovering" });
    store.getState().retryAuthentication();

    expect(store.getState().status).toBe("authenticating");
    expect(store.getState().retryGeneration).toBe(1);
  });

  it("explicit logout still clears credentials and publishes unauthenticated state", async () => {
    const storage = makeStorage({ patchbay_token: "t" });
    const api = makeApi();
    api.logout = vi.fn().mockResolvedValue(undefined);
    const onLogout = vi.fn();
    const store = createAuthStore({ api, storage, onLogout });

    store.setState({ user: fakeUser, status: "authenticated", isLoading: false });
    await store.getState().logout();

    expect(storage.snapshot().patchbay_token).toBeUndefined();
    expect(api.setToken).toHaveBeenCalledWith(null);
    expect(api.logout).not.toHaveBeenCalled();
    expect(onLogout).toHaveBeenCalledOnce();
    expect(store.getState().user).toBeNull();
    expect(store.getState().status).toBe("unauthenticated");
  });

  it("guest logout revokes the server session before clearing local state", async () => {
    const storage = makeStorage({ patchbay_token: "guest-t" });
    const api = makeApi();
    api.logout = vi.fn().mockResolvedValue(undefined);
    const store = createAuthStore({ api, storage });

    store.setState({
      user: { ...fakeUser, is_guest: true },
      status: "authenticated",
      isLoading: false,
    });
    await store.getState().logout();

    expect(api.logout).toHaveBeenCalledOnce();
    expect(store.getState().user).toBeNull();
  });

  it("createGuestSession persists the guest bearer and rejects non-guest users", async () => {
    const storage = makeStorage();
    const api = makeApi();
    const guestUser = { ...fakeUser, is_guest: true };
    api.createGuestSession = vi
      .fn()
      .mockResolvedValue({ token: "guest-t", user: guestUser });
    const onLogin = vi.fn();
    const store = createAuthStore({ api, storage, onLogin });

    const user = await store.getState().createGuestSession();

    expect(user).toEqual(guestUser);
    expect(storage.snapshot().patchbay_token).toBe("guest-t");
    expect(api.setToken).toHaveBeenCalledWith("guest-t");
    expect(onLogin).toHaveBeenCalledOnce();
    expect(store.getState().status).toBe("authenticated");

    api.createGuestSession = vi
      .fn()
      .mockResolvedValue({ token: "t", user: fakeUser });
    await expect(store.getState().createGuestSession()).rejects.toThrow();
  });
});
