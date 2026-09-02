/**
 * Mobile auth store — Zustand. Logic mirrors packages/core/auth/store.ts:
 *   - Token written ONLY on successful verifyCode
 *   - 401 → clear token; non-401 (5xx / network blip) → preserve token so
 *     the next launch can retry
 *   - logout = clear token + clear in-memory user + setToken(null)
 *
 * NOT shared with web/desktop (per Sharing Principles in root CLAUDE.md).
 * Storage backend is expo-secure-store (mobile only); web uses HttpOnly
 * cookies, desktop uses localStorage via StorageAdapter.
 */
import { create } from "zustand";
import type { User } from "@patchbay/core/types";
import { api, ApiError } from "./api";
import { clearToken, getToken, setToken } from "./secure-storage";
import {
  clearGuestCredentials,
  getGuestCredentials,
  saveGuestCredentials,
} from "./guest-storage";
import {
  isGuestToken,
  type GuestSession,
} from "./guest-auth";
import { queryClient } from "./query-client";
import { useWorkspaceStore } from "./workspace-store";

interface AuthState {
  user: User | null;
  isGuest: boolean;
  isLoading: boolean;
  initialize: () => Promise<void>;
  sendCode: (email: string) => Promise<void>;
  verifyCode: (email: string, code: string) => Promise<User>;
  continueAsGuest: () => Promise<User>;
  claimGuestSession: (sessionId?: string) => Promise<GuestSession>;
  logout: () => Promise<void>;
  /** Overwrite the in-memory user — call after PATCH /api/me so name/avatar
   *  edits land without a refetch. Server response is the source of truth. */
  setUser: (user: User) => void;
}

export const useAuthStore = create<AuthState>((set, get) => ({
  user: null,
  isGuest: false,
  isLoading: true,

  initialize: async () => {
    // Restore the persisted workspace slug alongside the auth token so the
    // entry redirect (app/index.tsx) can route directly to the last-used
    // workspace without flashing /select-workspace.
    await useWorkspaceStore.getState().restoreSlug();

    const token = await getToken();
    if (!token) {
      await Promise.allSettled([useWorkspaceStore.getState().clear()]);
      queryClient.clear();
      set({ isGuest: false, isLoading: false });
      return;
    }
    api.setToken(token);
    const tokenIsGuest = isGuestToken(token);
    try {
      const user = await api.getMe();
      set({
        user,
        isGuest:
          tokenIsGuest ||
          (user as User & { is_guest?: unknown }).is_guest === true,
        isLoading: false,
      });
    } catch (err) {
      // Only clear token on a genuine 401. Network blips / 5xx keep the
      // token so the next launch (or a manual refresh) can retry.
      if (err instanceof ApiError && err.status === 401) {
        await Promise.allSettled([
          clearToken(),
          useWorkspaceStore.getState().clear(),
        ]);
        queryClient.clear();
        api.setToken(null);
      }
      set({ user: null, isGuest: false, isLoading: false });
    }
  },

  sendCode: async (email) => {
    await api.sendCode(email);
  },

  verifyCode: async (email, code) => {
    const { token, user } = await api.verifyCode(email, code);
    await setToken(token);
    api.setToken(token);
    set({ user, isGuest: false });
    return user;
  },

  continueAsGuest: async () => {
    const { token, user, session_id: sessionId } = await api.createGuestAuth();
    if (!isGuestToken(token)) {
      throw new Error("The server did not return a guest token");
    }
    await saveGuestCredentials(token, sessionId);
    try {
      await setToken(token);
    } catch (error) {
      await clearGuestCredentials();
      throw error;
    }
    api.setToken(token);
    set({ user, isGuest: true, isLoading: false });
    return user;
  },

  claimGuestSession: async (sessionId) => {
    if (get().isGuest) {
      throw new Error("Formal login required to claim a guest session");
    }
    const credentials = await getGuestCredentials();
    const id = sessionId ?? credentials?.sessionId ?? null;
    if (!credentials || !id) {
      throw new Error("No guest session is available to claim");
    }
    const session = await api.claimGuestSession(id, credentials.token);
    await clearGuestCredentials();
    return session;
  },

  logout: async () => {
    let token: string | null = null;
    try {
      token = await getToken();
      // RevokeGuestOnLogout runs on this public endpoint. Clear local state in
      // finally so a network failure cannot strand the user in the app.
      if (token) await api.logout();
    } catch {
      // Local sign-out remains safe even when the server is unreachable.
    } finally {
      api.setToken(null);
      await Promise.allSettled([
        clearToken(),
        clearGuestCredentials(),
        useWorkspaceStore.getState().clear(),
      ]);
      queryClient.clear();
      set({ user: null, isGuest: false });
    }
  },

  setUser: (user) => set({ user }),
}));
