"use client";

import { useEffect } from "react";
import { useUser, useAuth } from "@clerk/nextjs";
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
  const { getToken, isSignedIn } = useAuth();

  useEffect(() => {
    if (!clerkLoaded) return;
    if (!isSignedIn || !clerkUser) {
      useAuthStore.getState().logout();
      return;
    }

    let cancelled = false;
    let retryIndex = 0;
    let retryTimer: ReturnType<typeof setTimeout> | undefined;
    const exchange = async () => {
      useAuthStore.setState({
        user: null,
        isLoading: true,
        status: retryIndex === 0 ? "authenticating" : "recovering",
      });
      try {
        const sessionToken = await getToken();
        if (!sessionToken) throw new Error("Clerk session token unavailable");
        await useAuthStore.getState().loginWithClerk(sessionToken);
      } catch {
        if (cancelled) return;
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
      if (retryTimer) clearTimeout(retryTimer);
    };
  }, [clerkLoaded, clerkUser?.id, getToken, isSignedIn]);

  return <>{children}</>;
}
