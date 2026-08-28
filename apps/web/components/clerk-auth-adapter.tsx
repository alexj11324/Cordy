"use client";

import { useEffect, useRef } from "react";
import { useUser, useAuth } from "@clerk/nextjs";
import { useAuthStore } from "@cordy/core/auth";

/**
 * Bridges Clerk's auth state into the existing Zustand AuthState store.
 *
 * All 182 files that consume `useAuthStore` or `useAuth` continue to work
 * unchanged — this component syncs Clerk's user/session into the same store
 * shape they already expect.
 *
 * Mount this component inside `<CoreProvider>` (web only).
 */
export function ClerkAuthAdapter({ children }: { children: React.ReactNode }) {
  const { user: clerkUser, isLoaded: clerkLoaded } = useUser();
  const { getToken, isSignedIn } = useAuth();
  const store = useAuthStore;
  const previousUserIdRef = useRef<string | null>(null);

  // Sync Clerk user → Zustand store whenever Clerk state changes
  useEffect(() => {
    if (!clerkLoaded) return;

    const storeState = store.getState();

    if (!isSignedIn || !clerkUser) {
      // User signed out — clear the store
      previousUserIdRef.current = null;
      storeState.setUser(null as any);
      return;
    }

    // Only update if the user actually changed (avoids unnecessary re-renders)
    if (previousUserIdRef.current === clerkUser.id) return;
    previousUserIdRef.current = clerkUser.id;

    // Map Clerk user → AuthUser shape that all consumers expect
    const authUser = {
      id: clerkUser.id,
      email: clerkUser.primaryEmailAddress?.emailAddress ?? "",
      name:
        clerkUser.fullName ||
        [clerkUser.firstName, clerkUser.lastName].filter(Boolean).join(" ") ||
        clerkUser.primaryEmailAddress?.emailAddress?.split("@")[0] ||
        "",
      avatarUrl: clerkUser.imageUrl || null,
      createdAt: clerkUser.createdAt?.toISOString() ?? new Date().toISOString(),
      // Clerk manages these server-side; keep them safe as empty
      emailVerified: true, // Clerk handles email verification
      avatarGenerated: false,
      roles: ["owner"],
      username: clerkUser.username || clerkUser.primaryEmailAddress?.emailAddress?.split("@")[0] || "",
    };

    storeState.setUser(authUser as any);
  }, [clerkLoaded, clerkUser, isSignedIn, store]);

  // Keep the API client's token getter current with Clerk sessions.
  // When the API client needs a token for a request, it calls getToken()
  // which returns the Clerk session JWT. The managed web identity boundary
  // authenticates that session before the request reaches the Rust API.
  useEffect(() => {
    if (!clerkLoaded || !isSignedIn) return;

    // Inject a token getter that the API client can use
    (window as any).__CLERK_GET_TOKEN__ = async () => {
      try {
        return await getToken();
      } catch {
        return null;
      }
    };

    return () => {
      delete (window as any).__CLERK_GET_TOKEN__;
    };
  }, [clerkLoaded, isSignedIn, getToken]);

  // Sync auth status so isLoading transitions correctly
  useEffect(() => {
    const storeState = store.getState();

    if (!clerkLoaded) {
      // Still loading — keep isLoading true
      return;
    }

    // Clerk has loaded. If signed out, status = "unauthenticated"
    // If signed in, the user effect above will set the user, which flips status
    if (!isSignedIn) {
      storeState.setUser(null as any);
    }
  }, [clerkLoaded, isSignedIn, store]);

  return <>{children}</>;
}
