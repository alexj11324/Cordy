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
    const storage = makeStorage({ orvilo_token: "t" });
    const api = makeApi();
    const store = createAuthStore({ api, storage });

    store.setState({ isLoading: true, status: "recovering" });
    store.getState().retryAuthentication();

    expect(store.getState().status).toBe("authenticating");
    expect(store.getState().retryGeneration).toBe(1);
  });

  it("explicit logout still clears credentials and publishes unauthenticated state", async () => {
    const storage = makeStorage({ orvilo_token: "t" });
    const api = makeApi();
    api.logout = vi.fn().mockResolvedValue(undefined);
    const onLogout = vi.fn();
    const store = createAuthStore({ api, storage, onLogout });

    store.setState({
      user: fakeUser,
      status: "authenticated",
      isLoading: false,
    });
    await store.getState().logout();

    expect(storage.snapshot().orvilo_token).toBeUndefined();
    expect(api.setToken).toHaveBeenCalledWith(null);
    expect(api.logout).not.toHaveBeenCalled();
    expect(onLogout).toHaveBeenCalledOnce();
    expect(store.getState().user).toBeNull();
    expect(store.getState().status).toBe("unauthenticated");
  });

  it("guest logout revokes the server session before clearing local state", async () => {
    const storage = makeStorage({ orvilo_token: "guest-t" });
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
    expect(storage.snapshot().orvilo_token).toBe("guest-t");
    expect(api.setToken).toHaveBeenCalledWith("guest-t");
    expect(onLogin).toHaveBeenCalledOnce();
    expect(store.getState().status).toBe("authenticated");

    api.createGuestSession = vi
      .fn()
      .mockResolvedValue({ token: "t", user: fakeUser });
    await expect(store.getState().createGuestSession()).rejects.toThrow();
  });

  it("confirmEmailChange replaces the native bearer and authenticated user", async () => {
    const storage = makeStorage({ orvilo_token: "old-token" });
    const api = makeApi();
    const updatedUser = { ...fakeUser, email: "alice.new@example.com" };
    api.confirmEmailChange = vi.fn().mockResolvedValue({
      token: "new-token",
      user: updatedUser,
    });
    const store = createAuthStore({ api, storage });
    store.setState({
      user: fakeUser,
      status: "authenticated",
      isLoading: false,
    });

    await expect(
      store.getState().confirmEmailChange("alice.new@example.com", "123456"),
    ).resolves.toEqual(updatedUser);

    expect(api.confirmEmailChange).toHaveBeenCalledWith(
      "alice.new@example.com",
      "123456",
    );
    expect(storage.snapshot().orvilo_token).toBe("new-token");
    expect(api.setToken).toHaveBeenCalledWith("new-token");
    expect(store.getState().user).toEqual(updatedUser);
  });

  it("confirmEmailChange keeps cookie auth out of token storage", async () => {
    const storage = makeStorage();
    const api = makeApi();
    const updatedUser = { ...fakeUser, email: "alice.cookie@example.com" };
    api.confirmEmailChange = vi.fn().mockResolvedValue({
      token: "response-token",
      user: updatedUser,
    });
    const store = createAuthStore({ api, storage, cookieAuth: true });

    await store
      .getState()
      .confirmEmailChange("alice.cookie@example.com", "123456");

    expect(storage.snapshot().orvilo_token).toBeUndefined();
    expect(api.setToken).toHaveBeenCalledWith(null);
    expect(store.getState().user).toEqual(updatedUser);
  });

  it("confirmEmailChange preserves the current session when confirmation fails", async () => {
    const storage = makeStorage({ orvilo_token: "old-token" });
    const api = makeApi();
    api.confirmEmailChange = vi
      .fn()
      .mockRejectedValue(new Error("invalid or expired code"));
    const store = createAuthStore({ api, storage });
    store.setState({
      user: fakeUser,
      status: "authenticated",
      isLoading: false,
    });

    await expect(
      store.getState().confirmEmailChange("alice.new@example.com", "000000"),
    ).rejects.toThrow("invalid or expired code");

    expect(storage.snapshot().orvilo_token).toBe("old-token");
    expect(api.setToken).not.toHaveBeenCalled();
    expect(store.getState().user).toEqual(fakeUser);
    expect(store.getState().status).toBe("authenticated");
  });
});
