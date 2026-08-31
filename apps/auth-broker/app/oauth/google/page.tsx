"use client";

import { Suspense, useEffect, useMemo, useRef, useState } from "react";
import { useClerk, useSignIn } from "@clerk/nextjs";
import { useSearchParams } from "next/navigation";
import { AuthShell } from "@/components/auth-shell";
import { useAuthMessages } from "@/lib/auth-messages";
import { registerDesktopGoogleAttempt } from "@/lib/broker-client";
import { readDesktopHandoffBinding } from "@/lib/desktop-handoff";
import { hasClerkOAuthReturn, readGoogleSso, startGoogleOAuth } from "@/lib/google-oauth";

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
    () => readDesktopHandoffBinding(searchParams),
    [searchParams],
  );
  const clerk = useClerk();
  const { signIn } = useSignIn();
  const { messages } = useAuthMessages();
  const attempted = useRef(false);
  const registeringAttempt = useRef(false);
  const clearingSession = useRef(false);
  const [attemptRegistered, setAttemptRegistered] = useState(false);
  const [error, setError] = useState(false);

  useEffect(() => {
    if (attempted.current) return;
    if (!binding) {
      setError(true);
      return;
    }
    if (!clerk.loaded) return;

    if (hasClerkOAuthReturn(searchParams, window.location.hash)) {
      attempted.current = true;
      window.location.replace(
        `/oauth/google/callback${window.location.search}${window.location.hash}`,
      );
      return;
    }

    if (!attemptRegistered) {
      if (registeringAttempt.current) return;
      registeringAttempt.current = true;
      void registerDesktopGoogleAttempt({
        state: binding.state,
        code_challenge: binding.codeChallenge,
      })
        .then(() => setAttemptRegistered(true))
        .catch(() => {
          registeringAttempt.current = false;
          setError(true);
        });
      return;
    }

    if (clerk.session) {
      if (searchParams.get("clerk_reset") === "1") {
        setError(true);
        return;
      }
      if (clearingSession.current) return;
      clearingSession.current = true;
      const resetQuery = new URLSearchParams(binding.query);
      resetQuery.set("clerk_reset", "1");
      void clerk
        .signOut({
          sessionId: clerk.session.id,
          redirectUrl: `/oauth/google?${resetQuery}`,
        })
        .catch(() => {
          clearingSession.current = false;
          setError(true);
        });
      return;
    }

    if (!readGoogleSso(signIn)) return;
    attempted.current = true;
    void startGoogleOAuth(signIn, {
      returnUrl: `/login?${binding.query}`,
      callbackUrl: `/oauth/google/callback?${binding.query}`,
      origin: window.location.origin,
    })
      .then(({ error: clerkError }) => {
        if (clerkError) setError(true);
      })
      .catch(() => setError(true));
  }, [attemptRegistered, binding, clerk, searchParams, signIn]);

  return (
    <AuthShell>
      <p role={error ? "alert" : "status"} aria-live="polite">
        {error
          ? binding
            ? messages.web.google_oauth.failed
            : messages.web.google_oauth.invalid_binding
          : messages.web.google_oauth.starting}
      </p>
    </AuthShell>
  );
}
