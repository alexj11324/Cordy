"use client";

import { Suspense, useEffect, useMemo, useRef, useState } from "react";
import { useAuth, useClerk, useSignIn } from "@clerk/nextjs";
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
import {
  useWebRouter,
  useWebSearchParams,
} from "@/platform/client-navigation";

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
  const { isSignedIn } = useAuth();
  const { signIn } = useSignIn();
  const router = useWebRouter();
  const { t } = useT("auth");
  const attempted = useRef(false);
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

    if (isSignedIn) {
      attempted.current = true;
      router.replace(returnUrl);
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
  }, [binding, clerk.loaded, isSignedIn, router, searchParams, signIn, t]);

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
