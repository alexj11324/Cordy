import { create } from "zustand";
import type { User, StorageAdapter } from "../types";
import { identify as identifyAnalytics, resetAnalytics } from "../analytics";
import type { ApiClient } from "../api/client";
import { setCurrentWorkspace } from "../platform/workspace-storage";

export interface AuthStoreOptions {
  api: ApiClient;
  storage: StorageAdapter;
  onLogin?: () => void;
  onLogout?: AuthLogoutHandler;
  /** When true, rely on HttpOnly cookies instead of localStorage for auth tokens. */
  cookieAuth?: boolean;
}

export type AuthLogoutOptions = {
  /** Prevent platform auth recovery when cleaning up a permanently rejected session. */
  rearmAuth?: boolean;
};

/** Optional promise that platform auth cleanup can await before re-authentication. */
export type AuthLogoutHandler = (
  serverLogout?: Promise<void>,
  options?: AuthLogoutOptions,
) => void | Promise<void>;

export type AuthStatus =
  | "authenticating"
  | "authenticated"
  | "unauthenticated"
  | "recovering";

export interface AuthState {
  user: User | null;
  isLoading: boolean;
  status: AuthStatus;
  retryGeneration: number;

  retryAuthentication: () => void;
  sendCode: (email: string) => Promise<void>;
  verifyCode: (email: string, code: string) => Promise<User>;
  loginWithGoogle: (code: string, redirectUri: string) => Promise<User>;
  createGuestSession: () => Promise<User>;
  loginWithToken: (token: string) => Promise<User>;
  /** Clears local auth state and resolves after a cookie/guest session is revoked. */
  logout: (options?: AuthLogoutOptions) => Promise<void>;
  setUser: (user: User) => void;
  refreshMe: () => Promise<void>;
}

export function createAuthStore(options: AuthStoreOptions) {
  const { api, storage, onLogin, onLogout, cookieAuth } = options;

  return create<AuthState>((set, get) => ({
    user: null,
    isLoading: true,
    status: "authenticating",
    retryGeneration: 0,

    retryAuthentication: () => {
      set((state) => ({
        isLoading: true,
        status: "authenticating",
        retryGeneration: state.retryGeneration + 1,
      }));
    },

    sendCode: async (email: string) => {
      await api.sendCode(email);
    },

    verifyCode: async (email: string, code: string) => {
      const { token, user } = await api.verifyCode(email, code);
      if (!cookieAuth) {
        // Token mode: persist for Electron / legacy.
        storage.setItem("patchbay_token", token);
        api.setToken(token);
      }
      onLogin?.();
      identifyAnalytics(user.id, { email: user.email, name: user.name });
      set({ user, isLoading: false, status: "authenticated" });
      return user;
    },

    loginWithGoogle: async (code: string, redirectUri: string) => {
      const { token, user } = await api.googleLogin(code, redirectUri);
      if (!cookieAuth) {
        storage.setItem("patchbay_token", token);
        api.setToken(token);
      }
      onLogin?.();
      identifyAnalytics(user.id, { email: user.email, name: user.name });
      set({ user, isLoading: false, status: "authenticated" });
      return user;
    },

    createGuestSession: async () => {
      const { token, user } = await api.createGuestSession();
      if (user.is_guest !== true) {
        throw new Error("server did not return a guest session");
      }
      // Guest auth is still token auth: the user is real and the bearer is
      // required for every subsequent workspace/onboarding API call.
      storage.setItem("patchbay_token", token);
      api.setToken(token);
      onLogin?.();
      identifyAnalytics(user.id, { email: user.email, name: user.name });
      set({ user, isLoading: false, status: "authenticated" });
      return user;
    },

    loginWithToken: async (token: string) => {
      storage.setItem("patchbay_token", token);
      api.setToken(token);
      const user = await api.getMe();
      onLogin?.();
      identifyAnalytics(user.id, { email: user.email, name: user.name });
      set({ user, isLoading: false, status: "authenticated" });
      return user;
    },

    logout: async (logoutOptions?: AuthLogoutOptions) => {
      const serverLogout =
        cookieAuth || get().user?.is_guest === true
          ? api.logout().catch(() => {})
          : Promise.resolve();
      const platformLogout = onLogout?.(serverLogout, logoutOptions);
      // Keep the promise so callers that are about to start a new exchange
      // or navigate away can serialize behind both server-side session
      // revocation and platform auth cleanup.
      storage.removeItem("patchbay_token");
      api.setToken(null);
      setCurrentWorkspace(null, null);
      resetAnalytics();
      set({ user: null, isLoading: false, status: "unauthenticated" });
      await Promise.all([serverLogout, platformLogout]);
    },

    setUser: (user: User) => {
      set({ user, isLoading: false, status: "authenticated" });
    },

    refreshMe: async () => {
      const user = await api.getMe();
      set({ user, isLoading: false, status: "authenticated" });
    },
  }));
}
