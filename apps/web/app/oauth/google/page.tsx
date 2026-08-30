"use client";

import { Suspense, useEffect, useMemo, useRef, useState } from "react";
import { useAuth, useSignIn } from "@clerk/nextjs";
import { ClerkAuthShell } from "@/components/clerk-auth-shell";
import { buildBrokerRoute } from "@/features/auth/broker-path";
import { readDesktopHandoffBinding } from "@/features/auth/desktop-handoff";
import { useT } from "@patchbay/views/i18n";
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
  const { isLoaded, isSignedIn, signOut } = useAuth();
  const { signIn } = useSignIn();
  const { t } = useT("auth");
  const signOutAttemptedFor = useRef<string | null>(null);
  const ssoAttemptedFor = useRef<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!binding) {
      setError(t(($) => $.web.google_oauth.invalid_binding));
      return;
    }
    if (!isLoaded) return;

    const currentPathname = window.location.pathname;
    const oauthStartUrl = `${buildBrokerRoute(
      currentPathname,
      "/oauth/google",
      "/oauth/google",
    )}?${binding.query}`;
    const failClosed = () => {
      setError(t(($) => $.web.google_oauth.failed));
    };

    // A browser session from the web app must not silently authorize a new
    // desktop login. Clerk also rejects a second sign-in attempt with
    // `session_exists`, before Google can show its account chooser. Clear the
    // browser-side Clerk sessions, then return to this exact PKCE/state-bound
    // entry URL and start a fresh provider attempt.
    if (isSignedIn) {
      if (signOutAttemptedFor.current === binding.query) return;
      signOutAttemptedFor.current = binding.query;
      void signOut({ redirectUrl: oauthStartUrl }).catch(failClosed);
      return;
    }

    if (ssoAttemptedFor.current === binding.query) return;
    ssoAttemptedFor.current = binding.query;

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
    // Existing Google users stay on sign-in. The callback transfers a new
    // external account to sign-up when Clerk marks this attempt transferable.
    void signIn
      .sso({
        strategy: "oauth_google",
        redirectUrl: returnUrl,
        redirectCallbackUrl: callbackUrl,
        oidcPrompt: "select_account",
      })
      .then(({ error: clerkError }) => {
        if (clerkError) failClosed();
      })
      .catch(failClosed);
  }, [binding, isLoaded, isSignedIn, signIn, signOut, t]);

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
