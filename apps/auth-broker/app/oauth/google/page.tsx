"use client";

import {
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useClerk, useSignIn } from "@clerk/nextjs";
import { useSearchParams } from "next/navigation";
import { AuthShell } from "@/components/auth-shell";
import { useAuthMessages } from "@/lib/auth-messages";
import { registerDesktopGoogleAttempt } from "@/lib/broker-client";
import { readDesktopHandoffBinding } from "@/lib/desktop-handoff";
import {
  GOOGLE_OAUTH_START_TIMEOUT_MS,
  GoogleOAuthStartTimeoutError,
  hasClerkOAuthReturn,
  readGoogleSso,
  startGoogleOAuth,
  withGoogleOAuthStartTimeout,
} from "@/lib/google-oauth";

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
  const startTimeout = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [attemptRegistered, setAttemptRegistered] = useState(false);
  const [error, setError] = useState<"failed" | "timeout" | null>(null);
  const clearStartTimeout = useCallback(() => {
    if (startTimeout.current !== null) {
      clearTimeout(startTimeout.current);
      startTimeout.current = null;
    }
  }, []);

  useEffect(
    () => () => {
      if (startTimeout.current !== null) {
        clearTimeout(startTimeout.current);
        startTimeout.current = null;
      }
    },
    [],
  );

  useEffect(() => {
    if (attempted.current || error) return;
    if (!binding) {
      setError("failed");
      return;
    }

    if (startTimeout.current === null) {
      startTimeout.current = setTimeout(() => {
        startTimeout.current = null;
        if (!attempted.current) setError("timeout");
      }, GOOGLE_OAUTH_START_TIMEOUT_MS);
    }
    if (!clerk.loaded) return;

    if (hasClerkOAuthReturn(searchParams, window.location.hash)) {
      attempted.current = true;
      clearStartTimeout();
      window.location.replace(
        `/oauth/google/callback${window.location.search}${window.location.hash}`,
      );
      return;
    }

    if (!attemptRegistered) {
      if (registeringAttempt.current) return;
      registeringAttempt.current = true;
      void withGoogleOAuthStartTimeout(
        registerDesktopGoogleAttempt({
          state: binding.state,
          code_challenge: binding.codeChallenge,
        }),
      )
        .then(() => setAttemptRegistered(true))
        .catch((caught) => {
          registeringAttempt.current = false;
          clearStartTimeout();
          setError(
            caught instanceof GoogleOAuthStartTimeoutError ? "timeout" : "failed",
          );
        });
      return;
    }

    if (clerk.session) {
      if (searchParams.get("clerk_reset") === "1") {
        clearStartTimeout();
        setError("failed");
        return;
      }
      if (clearingSession.current) return;
      clearingSession.current = true;
      const resetQuery = new URLSearchParams(binding.query);
      resetQuery.set("clerk_reset", "1");
      void withGoogleOAuthStartTimeout(
        clerk.signOut({
          sessionId: clerk.session.id,
          redirectUrl: `/oauth/google?${resetQuery}`,
        }),
      ).catch((caught) => {
        clearingSession.current = false;
        clearStartTimeout();
        setError(
          caught instanceof GoogleOAuthStartTimeoutError ? "timeout" : "failed",
        );
      });
      return;
    }

    if (!readGoogleSso(signIn)) return;
    attempted.current = true;
    clearStartTimeout();
    void withGoogleOAuthStartTimeout(
      startGoogleOAuth(signIn, {
        returnUrl: `/login?${binding.query}`,
        callbackUrl: `/oauth/google/callback?${binding.query}`,
        origin: window.location.origin,
      }),
    )
      .then(({ error: clerkError }) => {
        if (clerkError) setError("failed");
      })
      .catch((caught) =>
        setError(
          caught instanceof GoogleOAuthStartTimeoutError ? "timeout" : "failed",
        ),
      );
  }, [
    attemptRegistered,
    binding,
    clearStartTimeout,
    clerk,
    error,
    searchParams,
    signIn,
  ]);

  return (
    <AuthShell>
      <p role={error ? "alert" : "status"} aria-live="polite">
        {error
          ? binding
            ? error === "timeout"
              ? messages.web.google_oauth.timeout
              : messages.web.google_oauth.failed
            : messages.web.google_oauth.invalid_binding
          : messages.web.google_oauth.starting}
      </p>
      {error && binding && (
        <button type="button" onClick={() => window.location.reload()}>
          {messages.web.google_oauth.retry}
        </button>
      )}
    </AuthShell>
  );
}
