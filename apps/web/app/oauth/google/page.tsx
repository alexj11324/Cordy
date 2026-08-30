"use client";

import { Suspense, useEffect, useMemo, useRef, useState } from "react";
import { useClerk, useSignIn } from "@clerk/nextjs";
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
    () =>
      readDesktopHandoffBinding(searchParams),
    [searchParams],
  );
  const clerk = useClerk();
  const { signIn } = useSignIn();
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
    attempted.current = true;

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
        if (clerkError) {
          setError(t(($) => $.web.google_oauth.failed));
        }
      })
      .catch(() => {
        setError(t(($) => $.web.google_oauth.failed));
      });
  }, [binding, clerk.loaded, signIn, t]);

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
