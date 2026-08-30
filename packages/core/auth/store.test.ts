import { describe, expect, it, vi } from "vitest";
import type { ApiClient } from "../api/client";
import type { StorageAdapter, User } from "../types";
import { createAuthStore } from "./store";
import type { AuthLogoutOptions } from "./store";

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

  it("retains a handed-off token when user hydration fails transiently", async () => {
    const storage = makeStorage();
    const api = {
      getMe: vi.fn().mockRejectedValue(new TypeError("temporarily offline")),
      setToken: vi.fn(),
    } as unknown as ApiClient;
    const store = createAuthStore({ api, storage });

    await expect(
      store.getState().loginWithToken("redeemed-session-token"),
    ).rejects.toThrow("temporarily offline");

    expect(storage.snapshot().patchbay_token).toBe("redeemed-session-token");
    expect(api.setToken).toHaveBeenCalledWith("redeemed-session-token");
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

  it("waits for platform auth cleanup before logout resolves", async () => {
    let finishPlatformLogout: (() => void) | undefined;
    const platformLogout = new Promise<void>((resolve) => {
      finishPlatformLogout = resolve;
    });
    const onLogout = vi.fn(() => platformLogout);
    const store = createAuthStore({
      api: makeApi(),
      storage: makeStorage({ patchbay_token: "t" }),
      onLogout,
    });

    let settled = false;
    const logout = store
      .getState()
      .logout()
      .then(() => {
        settled = true;
      });

    expect(onLogout).toHaveBeenCalledOnce();
    await Promise.resolve();
    expect(settled).toBe(false);

    finishPlatformLogout?.();
    await logout;
    expect(settled).toBe(true);
  });

  it("exchanges a Clerk session for the UUID-backed cookie session", async () => {
    const storage = makeStorage();
    const onLogin = vi.fn();
    const api = {
      clerkLogin: vi.fn().mockResolvedValue({
        token: "patchbay-token",
        user: fakeUser,
      }),
      setToken: vi.fn(),
      setTokenProvider: vi.fn(),
    } as unknown as ApiClient;
    const store = createAuthStore({
      api,
      storage,
      cookieAuth: true,
      onLogin,
    });
    const signal = new AbortController().signal;

    await expect(
      store.getState().loginWithClerk("clerk-session", signal),
    ).resolves.toEqual(fakeUser);

    expect(api.clerkLogin).toHaveBeenCalledWith("clerk-session", signal);
    expect(api.setTokenProvider).toHaveBeenCalledWith(null);
    expect(api.setToken).toHaveBeenCalledWith(null);
    expect(storage.snapshot()).toEqual({});
    expect(onLogin).toHaveBeenCalledOnce();
    expect(store.getState()).toMatchObject({
      user: fakeUser,
      isLoading: false,
      status: "authenticated",
    });
  });

  it("does not publish a stale Clerk exchange after cancellation", async () => {
    const storage = makeStorage();
    const onLogin = vi.fn();
    const api = {
      clerkLogin: vi.fn().mockResolvedValue({
        token: "patchbay-token",
        user: fakeUser,
      }),
      setToken: vi.fn(),
      setTokenProvider: vi.fn(),
    } as unknown as ApiClient;
    const store = createAuthStore({ api, storage, cookieAuth: true, onLogin });
    const controller = new AbortController();
    controller.abort();

    await expect(
      store.getState().loginWithClerk("clerk-session", controller.signal),
    ).rejects.toMatchObject({ name: "AbortError" });
    expect(api.setTokenProvider).not.toHaveBeenCalled();
    expect(api.setToken).not.toHaveBeenCalled();
    expect(onLogin).not.toHaveBeenCalled();
    expect(store.getState().user).toBeNull();
  });

  it("keeps cookie logout pending until the server revocation finishes", async () => {
    let resolveLogout: (() => void) | undefined;
    const logout = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveLogout = resolve;
        }),
    );
    const api = {
      logout,
      setToken: vi.fn(),
    } as unknown as ApiClient;
    const store = createAuthStore({
      api,
      storage: makeStorage(),
      cookieAuth: true,
    });

    let settled = false;
    const pending = store.getState().logout().then(() => {
      settled = true;
    });
    await Promise.resolve();
    expect(logout).toHaveBeenCalledOnce();
    expect(settled).toBe(false);

    resolveLogout?.();
    await pending;
    expect(settled).toBe(true);
  });

  it("passes the server revocation barrier to platform logout", async () => {
    let resolveServerLogout!: () => void;
    const api = {
      logout: vi.fn(
        () =>
          new Promise<void>((resolve) => {
            resolveServerLogout = resolve;
          }),
      ),
      setToken: vi.fn(),
    } as unknown as ApiClient;
    let receivedBarrier: Promise<void> | undefined;
    let receivedOptions: AuthLogoutOptions | undefined;
    const onLogout = vi.fn(
      (serverLogout?: Promise<void>, options?: AuthLogoutOptions) => {
        receivedBarrier = serverLogout;
        receivedOptions = options;
      },
    );
    const store = createAuthStore({
      api,
      storage: makeStorage(),
      cookieAuth: true,
      onLogout,
    });
    store.setState({ user: fakeUser, status: "authenticated", isLoading: false });

    let settled = false;
    const logoutOptions = { rearmAuth: false };
    const pending = store.getState().logout(logoutOptions).then(() => {
      settled = true;
    });
    expect(receivedBarrier).toBeInstanceOf(Promise);
    expect(receivedOptions).toEqual(logoutOptions);
    await Promise.resolve();
    expect(settled).toBe(false);

    resolveServerLogout();
    await pending;
    expect(settled).toBe(true);
  });

  it("revokes a server-backed guest session on logout", () => {
    const storage = makeStorage({ patchbay_token: "pbg_guest-token" });
    const api = {
      logout: vi.fn().mockResolvedValue(undefined),
      setToken: vi.fn(),
    } as unknown as ApiClient;
    const store = createAuthStore({ api, storage });

    store.setState({
      user: { ...fakeUser, id: "guest-1", is_guest: true },
      status: "authenticated",
      isLoading: false,
    });
    store.getState().logout();

    expect(api.logout).toHaveBeenCalledOnce();
    expect(api.setToken).toHaveBeenCalledWith(null);
    expect(storage.snapshot().patchbay_token).toBeUndefined();
  });
});
