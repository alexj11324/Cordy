"use client";

import { Suspense, useEffect, useMemo, useRef, useState } from "react";
import { useSignIn } from "@clerk/nextjs";
import { useSearchParams } from "next/navigation";
import { ClerkAuthShell } from "@/components/clerk-auth-shell";
import { readDesktopHandoffBinding } from "@/features/auth/desktop-handoff";
import { useT } from "@patchbay/views/i18n";

export default function GoogleOAuthPage() {
  return (
    <Suspense>
      <GoogleOAuthContent />
    </Suspense>
  );
}

function GoogleOAuthContent() {
  const searchParams = useSearchParams();
  const binding = useMemo(
    () =>
      readDesktopHandoffBinding(
        searchParams,
        process.env.NEXT_PUBLIC_DESKTOP_APP_ORIGIN,
      ),
    [searchParams],
  );
  const { signIn } = useSignIn();
  const { t } = useT("auth");
  const attempted = useRef(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (attempted.current) return;
    attempted.current = true;
    if (!binding) {
      setError(t(($) => $.web.google_oauth.invalid_binding));
      return;
    }

    const returnUrl = `/login?${binding.query}`;
    const callbackUrl = `/oauth/google/callback?${binding.query}`;
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
  }, [binding, signIn, t]);

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
