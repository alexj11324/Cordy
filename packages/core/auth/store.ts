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
  loginWithClerk: (sessionToken: string, signal?: AbortSignal) => Promise<User>;
  createGuestSession: () => Promise<User>;
  loginWithToken: (token: string) => Promise<User>;
  /** Clears local auth state and resolves after a cookie session is revoked. */
  logout: (options?: AuthLogoutOptions) => Promise<void>;
  setUser: (user: User) => void;
  refreshMe: () => Promise<void>;
}

export function createAuthStore(options: AuthStoreOptions) {
  const { api, storage, onLogin, onLogout, cookieAuth } = options;
  // A logout can involve asynchronous platform cleanup (for Desktop this
  // stops the local daemon and clears its profile). Track auth transitions so
  // an overlapping login cannot be cleared by the older logout completion.
  let transitionGeneration = 0;

  const beginTransition = () => {
    transitionGeneration += 1;
    return transitionGeneration;
  };

  const transitionWasSuperseded = (generation: number) =>
    generation !== transitionGeneration;

  const supersededTransitionError = () => {
    const error = new Error("authentication transition superseded");
    error.name = "AbortError";
    return error;
  };

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
      const generation = beginTransition();
      const { token, user } = await api.verifyCode(email, code);
      if (transitionWasSuperseded(generation)) {
        throw supersededTransitionError();
      }
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

    loginWithClerk: async (sessionToken: string, signal?: AbortSignal) => {
      const generation = beginTransition();
      const { token, user } = await api.clerkLogin(sessionToken, signal);
      if (signal?.aborted || transitionWasSuperseded(generation)) {
        const error = new Error("Clerk session exchange aborted");
        error.name = "AbortError";
        throw error;
      }
      // The Clerk token is only an input to the exchange. Every subsequent
      // API and WebSocket request uses the HttpOnly Patchbay session cookie.
      api.setTokenProvider(null);
      if (cookieAuth) {
        api.setToken(null);
      } else {
        storage.setItem("patchbay_token", token);
        api.setToken(token);
      }
      onLogin?.();
      identifyAnalytics(user.id, { email: user.email, name: user.name });
      set({ user, isLoading: false, status: "authenticated" });
      return user;
    },

    createGuestSession: async () => {
      const generation = beginTransition();
      const { token, user } = await api.createGuestSession();
      if (transitionWasSuperseded(generation)) {
        throw supersededTransitionError();
      }
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
      const generation = beginTransition();
      storage.setItem("patchbay_token", token);
      api.setToken(token);
      const user = await api.getMe();
      if (transitionWasSuperseded(generation)) {
        if (storage.getItem("patchbay_token") === token) {
          storage.removeItem("patchbay_token");
          api.setToken(null);
        }
        throw supersededTransitionError();
      }
      onLogin?.();
      identifyAnalytics(user.id, { email: user.email, name: user.name });
      set({ user, isLoading: false, status: "authenticated" });
      return user;
    },

    logout: async (logoutOptions?: AuthLogoutOptions) => {
      const generation = beginTransition();
      const serverLogout =
        cookieAuth || get().user?.is_guest === true
          ? api.logout().catch(() => {})
          : Promise.resolve();
      const platformLogout = onLogout?.(serverLogout, logoutOptions);

      // Desktop's platform cleanup is a security boundary, not a best-effort
      // side effect. Wait for it before publishing an unauthenticated state;
      // if it fails, the caller remains signed in and can retry. A newer login
      // wins the transition and must not be cleared by this older logout.
      if (platformLogout) {
        await Promise.all([serverLogout, platformLogout]);
        if (transitionWasSuperseded(generation)) return;
      }

      // Keep the promise so callers that are about to start a new Clerk
      // exchange or navigate away can serialize behind both server-side
      // session revocation and platform auth cleanup (for example Clerk).
      storage.removeItem("patchbay_token");
      api.setToken(null);
      setCurrentWorkspace(null, null);
      resetAnalytics();
      set({ user: null, isLoading: false, status: "unauthenticated" });
      if (!platformLogout) await serverLogout;
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
