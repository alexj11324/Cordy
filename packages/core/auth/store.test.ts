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

  it("persists a real guest bearer and publishes the server user", async () => {
    const storage = makeStorage();
    const onLogin = vi.fn();
    const guestUser = { ...fakeUser, id: "guest-1", is_guest: true };
    const api = {
      createGuestSession: vi.fn().mockResolvedValue({
        token: "pbg_guest-token",
        user: guestUser,
      }),
      setToken: vi.fn(),
    } as unknown as ApiClient;
    const store = createAuthStore({ api, storage, onLogin });

    await expect(store.getState().createGuestSession()).resolves.toEqual(guestUser);

    expect(api.createGuestSession).toHaveBeenCalledOnce();
    expect(storage.snapshot()).toEqual({ patchbay_token: "pbg_guest-token" });
    expect(api.setToken).toHaveBeenCalledWith("pbg_guest-token");
    expect(onLogin).toHaveBeenCalledOnce();
    expect(store.getState()).toMatchObject({
      user: guestUser,
      isLoading: false,
      status: "authenticated",
    });
  });

  it("rejects a non-guest response without persisting its token", async () => {
    const storage = makeStorage();
    const api = {
      createGuestSession: vi.fn().mockResolvedValue({
        token: "unexpected-token",
        user: fakeUser,
      }),
      setToken: vi.fn(),
    } as unknown as ApiClient;
    const store = createAuthStore({ api, storage });

    await expect(store.getState().createGuestSession()).rejects.toThrow(
      "server did not return a guest session",
    );
    expect(storage.snapshot()).toEqual({});
    expect(api.setToken).not.toHaveBeenCalled();
  });

  it("explicit logout still clears credentials and publishes unauthenticated state", () => {
    const storage = makeStorage({ patchbay_token: "t" });
    const api = makeApi();
    const onLogout = vi.fn();
    const store = createAuthStore({ api, storage, onLogout });

    store.setState({ user: fakeUser, status: "authenticated", isLoading: false });
    store.getState().logout();

    expect(storage.snapshot().patchbay_token).toBeUndefined();
    expect(api.setToken).toHaveBeenCalledWith(null);
    expect(onLogout).toHaveBeenCalledOnce();
    expect(store.getState().user).toBeNull();
    expect(store.getState().status).toBe("unauthenticated");
  });
});
