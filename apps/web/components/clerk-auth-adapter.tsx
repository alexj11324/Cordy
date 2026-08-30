"use client";

import { useEffect, useRef } from "react";
import { useUser, useAuth } from "@clerk/nextjs";
import { ApiError } from "@patchbay/core/api";
import { useAuthStore } from "@patchbay/core/auth";

const RETRY_DELAYS_MS = [1_000, 2_000, 4_000, 8_000, 16_000, 30_000] as const;

/**
 * Bridges Clerk's auth state into the existing Zustand AuthState store.
 *
 * All 182 files that consume `useAuthStore` or `useAuth` continue to work
 * unchanged — this component syncs Clerk's user/session into the same store
 * shape they already expect.
 *
 * Mount this component inside `<CoreProvider>` (web only).
 */
export function ClerkAuthAdapter({
  children,
}: {
  children: React.ReactNode;
}) {
  const { user: clerkUser, isLoaded: clerkLoaded } = useUser();
  const { getToken, isSignedIn, signOut } = useAuth();
  const logoutBarrierRef = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    if (!clerkLoaded) return;
    if (!isSignedIn || !clerkUser) {
      logoutBarrierRef.current = Promise.resolve(
        useAuthStore.getState().logout(),
      );
      return;
    }

    const previousLogout = logoutBarrierRef.current;
    let cancelled = false;
    let retryIndex = 0;
    let retryTimer: ReturnType<typeof setTimeout> | undefined;
    let abortController: AbortController | undefined;
    const exchange = async () => {
      await previousLogout;
      if (cancelled) return;
      const controller = new AbortController();
      abortController = controller;
      useAuthStore.setState({
        user: null,
        isLoading: true,
        status: retryIndex === 0 ? "authenticating" : "recovering",
      });
      try {
        const sessionToken = await getToken();
        if (!sessionToken) throw new Error("Clerk session token unavailable");
        await useAuthStore
          .getState()
          .loginWithClerk(sessionToken, controller.signal);
      } catch (error) {
        if (cancelled || controller.signal.aborted) return;
        const status = error instanceof ApiError ? error.status : undefined;
        const isPermanentRejection =
          status !== undefined &&
          status >= 400 &&
          status < 500 &&
          status !== 408 &&
          status !== 429;
        if (isPermanentRejection) {
          // A rejected identity cannot recover by retrying the same Clerk
          // session. Clear the Patchbay session and Clerk identity so the
          // user can take an actionable sign-in path instead of seeing a
          // blank recovering shell forever.
          logoutBarrierRef.current = useAuthStore.getState().logout();
          await logoutBarrierRef.current;
          if (cancelled) return;
          void signOut().catch(() => {});
          return;
        }
        const delay =
          RETRY_DELAYS_MS[Math.min(retryIndex, RETRY_DELAYS_MS.length - 1)] ??
          30_000;
        retryIndex += 1;
        useAuthStore.setState({
          user: null,
          isLoading: true,
          status: "recovering",
        });
        retryTimer = setTimeout(() => void exchange(), delay);
      }
    };
    void exchange();
    return () => {
      cancelled = true;
      abortController?.abort();
      if (retryTimer) clearTimeout(retryTimer);
    };
  }, [clerkLoaded, clerkUser?.id, getToken, isSignedIn, signOut]);

  return <>{children}</>;
}
