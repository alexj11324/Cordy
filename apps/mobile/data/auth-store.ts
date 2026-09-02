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
import {
  clearLegacyGuestCredentials,
  clearToken,
  getToken,
  setToken,
} from "./secure-storage";
import { queryClient } from "./query-client";
import { useWorkspaceStore } from "./workspace-store";

interface AuthState {
  user: User | null;
  isLoading: boolean;
  initialize: () => Promise<void>;
  sendCode: (email: string) => Promise<void>;
  verifyCode: (email: string, code: string) => Promise<User>;
  logout: () => Promise<void>;
  /** Overwrite the in-memory user — call after PATCH /api/me so name/avatar
   *  edits land without a refetch. Server response is the source of truth. */
  setUser: (user: User) => void;
}

export const useAuthStore = create<AuthState>((set) => ({
  user: null,
  isLoading: true,

  initialize: async () => {
    // Restore the persisted workspace slug alongside the auth token so the
    // entry redirect (app/index.tsx) can route directly to the last-used
    // workspace without flashing /select-workspace.
    await useWorkspaceStore.getState().restoreSlug();

    const token = await getToken();
    if (!token) {
      await Promise.allSettled([
        clearLegacyGuestCredentials(),
        useWorkspaceStore.getState().clear(),
      ]);
      queryClient.clear();
      set({ isLoading: false });
      return;
    }
    // Guest mode is Desktop-local-only. Never restore a bearer created by the
    // removed mobile Guest experiment, even if an older build persisted one.
    if (token.startsWith("pbg_")) {
      api.setToken(null);
      await Promise.allSettled([
        clearToken(),
        clearLegacyGuestCredentials(),
        useWorkspaceStore.getState().clear(),
      ]);
      queryClient.clear();
      set({ user: null, isLoading: false });
      return;
    }
    api.setToken(token);
    try {
      const user = await api.getMe();
      set({ user, isLoading: false });
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
      set({ user: null, isLoading: false });
    }
  },

  sendCode: async (email) => {
    await api.sendCode(email);
  },

  verifyCode: async (email, code) => {
    const { token, user } = await api.verifyCode(email, code);
    await setToken(token);
    api.setToken(token);
    set({ user });
    return user;
  },

  logout: async () => {
    let token: string | null = null;
    try {
      token = await getToken();
      // Clear local state in finally so a network failure cannot strand the
      // user in the app.
      if (token) await api.logout();
    } catch {
      // Local sign-out remains safe even when the server is unreachable.
    } finally {
      api.setToken(null);
      await Promise.allSettled([
        clearToken(),
        clearLegacyGuestCredentials(),
        useWorkspaceStore.getState().clear(),
      ]);
      queryClient.clear();
      set({ user: null });
    }
  },

  setUser: (user) => set({ user }),
}));
