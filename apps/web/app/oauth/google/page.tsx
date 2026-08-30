"use client";

import { Suspense, useEffect, useMemo, useRef, useState } from "react";
import { useClerk, useSignIn } from "@clerk/nextjs";
import { ClerkAuthShell } from "@/components/clerk-auth-shell";
import { buildBrokerRoute } from "@/features/auth/broker-path";
import { readDesktopHandoffBinding } from "@/features/auth/desktop-handoff";
import {
  canStartGoogleOAuth,
  googleOAuthCallbackHref,
  hasClerkOAuthReturn,
  startGoogleOAuth,
} from "@/features/auth/google-oauth";
import { useT } from "@patchbay/views/i18n";
import { api } from "@patchbay/core/api";
import { useWebSearchParams } from "@/platform/client-navigation";

export default function GoogleOAuthPage() {
  return (
    <Suspense>
      <GoogleOAuthContent />
    </Suspense>
  );
}

function GoogleOAuthContent() {
  const searchParams = useWebSearchParams();
  const binding = useMemo(
    () => readDesktopHandoffBinding(searchParams),
    [searchParams],
  );
  const clerk = useClerk();
  const { signIn } = useSignIn();
  const { t } = useT("auth");
  const attempted = useRef(false);
  const registeringAttempt = useRef(false);
  const clearingSession = useRef(false);
  const [attemptRegistered, setAttemptRegistered] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (attempted.current) return;
    if (!binding) {
      setError(t(($) => $.web.google_oauth.invalid_binding));
      return;
    }
    if (!clerk.loaded) return;

    const currentPathname = window.location.pathname;
    const returnUrl = `${buildBrokerRoute(
      currentPathname,
      "/oauth/google",
      "/login",
    )}?${binding.query}`;
    const callbackUrl = `${buildBrokerRoute(
      currentPathname,
      "/oauth/google",
      "/oauth/google/callback",
    )}?${binding.query}`;

    if (hasClerkOAuthReturn(searchParams, window.location.hash)) {
      attempted.current = true;
      window.location.replace(
        googleOAuthCallbackHref({
          pathname: currentPathname,
          search: window.location.search,
          hash: window.location.hash,
        }),
      );
      return;
    }

    if (!attemptRegistered) {
      if (registeringAttempt.current) return;
      registeringAttempt.current = true;
      void api
        .registerDesktopGoogleAttempt(binding.state, binding.codeChallenge)
        .then(({ registered }) => {
          if (!registered) throw new Error("Desktop Google OAuth attempt unavailable");
          setAttemptRegistered(true);
        })
        .catch(() => {
          registeringAttempt.current = false;
          setError(t(($) => $.web.google_oauth.failed));
        });
      return;
    }

    // An ambient Clerk cookie is not proof that this desktop-initiated Google
    // attempt completed. Clear only the active session, then restart from a
    // canonical URL so the next document can safely begin Google SSO.
    if (clerk.session) {
      if (searchParams.get("clerk_reset") === "1") {
        setError(t(($) => $.web.google_oauth.failed));
        return;
      }
      if (clearingSession.current) return;
      clearingSession.current = true;
      const resetQuery = new URLSearchParams(binding.query);
      resetQuery.set("clerk_reset", "1");
      const restartUrl = new URL(
        `${buildBrokerRoute(
          currentPathname,
          "/oauth/google",
          "/oauth/google",
        )}?${resetQuery}`,
        window.location.origin,
      ).href;
      void clerk
        .signOut({ sessionId: clerk.session.id, redirectUrl: restartUrl })
        .catch(() => {
          clearingSession.current = false;
          setError(t(($) => $.web.google_oauth.failed));
        });
      return;
    }

    if (!canStartGoogleOAuth(signIn)) return;

    attempted.current = true;
    // Existing Google users stay on sign-in. The callback transfers a new
    // external account to sign-up when Clerk marks this attempt transferable.
    void startGoogleOAuth(signIn, {
      returnUrl,
      callbackUrl,
      origin: window.location.origin,
    })
      .then(({ error: clerkError }) => {
        if (clerkError) {
          setError(t(($) => $.web.google_oauth.failed));
        }
      })
      .catch(() => {
        setError(t(($) => $.web.google_oauth.failed));
      });
  }, [attemptRegistered, binding, clerk, searchParams, signIn, t]);

  return (
    <ClerkAuthShell>
      <p
        role={error ? "alert" : "status"}
        aria-live="polite"
        className={
          error
            ? "text-body text-destructive"
            : "text-body text-muted-foreground"
        }
      >
        {error ?? t(($) => $.web.google_oauth.starting)}
      </p>
    </ClerkAuthShell>
  );
}
