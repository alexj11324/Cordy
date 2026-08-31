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
import { ClerkAuthShell } from "@/components/clerk-auth-shell";
import { buildBrokerRoute } from "@/features/auth/broker-path";
import { readDesktopHandoffBinding } from "@/features/auth/desktop-handoff";
import {
  canStartGoogleOAuth,
  googleOAuthCallbackHref,
  GOOGLE_OAUTH_START_TIMEOUT_MS,
  GoogleOAuthStartTimeoutError,
  hasClerkOAuthReturn,
  startGoogleOAuth,
  withGoogleOAuthStartTimeout,
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
  const startTimeout = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [attemptRegistered, setAttemptRegistered] = useState(false);
  const [error, setError] = useState<string | null>(null);

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
      setError(t(($) => $.web.google_oauth.invalid_binding));
      return;
    }

    if (startTimeout.current === null) {
      startTimeout.current = setTimeout(() => {
        startTimeout.current = null;
        if (!attempted.current) {
          setError(t(($) => $.web.google_oauth.timeout));
        }
      }, GOOGLE_OAUTH_START_TIMEOUT_MS);
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
      clearStartTimeout();
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
      void withGoogleOAuthStartTimeout(
        api.registerDesktopGoogleAttempt(binding.state, binding.codeChallenge),
      )
        .then(({ registered }) => {
          if (!registered) throw new Error("Desktop Google OAuth attempt unavailable");
          setAttemptRegistered(true);
        })
        .catch((caught) => {
          registeringAttempt.current = false;
          clearStartTimeout();
          setError(
            t(($) =>
              caught instanceof GoogleOAuthStartTimeoutError
                ? $.web.google_oauth.timeout
                : $.web.google_oauth.failed,
            ),
          );
        });
      return;
    }

    // An ambient Clerk cookie is not proof that this desktop-initiated Google
    // attempt completed. Clear every session on this Clerk client, then
    // restart from a canonical URL so another cached session cannot become
    // active before the next document begins Google SSO.
    if (clerk.session) {
      if (searchParams.get("clerk_reset") === "1") {
        clearStartTimeout();
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
      void withGoogleOAuthStartTimeout(
        clerk.signOut({ redirectUrl: restartUrl }),
      ).catch((caught) => {
        clearingSession.current = false;
        clearStartTimeout();
        setError(
          t(($) =>
            caught instanceof GoogleOAuthStartTimeoutError
              ? $.web.google_oauth.timeout
              : $.web.google_oauth.failed,
          ),
        );
      });
      return;
    }

    if (!canStartGoogleOAuth(signIn)) return;

    attempted.current = true;
    clearStartTimeout();
    // Existing Google users stay on sign-in. The callback transfers a new
    // external account to sign-up when Clerk marks this attempt transferable.
    void withGoogleOAuthStartTimeout(
      startGoogleOAuth(signIn, {
        returnUrl,
        callbackUrl,
        origin: window.location.origin,
      }),
    )
      .then(({ error: clerkError }) => {
        if (clerkError) {
          setError(t(($) => $.web.google_oauth.failed));
        }
      })
      .catch((caught) => {
        setError(
          t(($) =>
            caught instanceof GoogleOAuthStartTimeoutError
              ? $.web.google_oauth.timeout
              : $.web.google_oauth.failed,
          ),
        );
      });
  }, [
    attemptRegistered,
    binding,
    clearStartTimeout,
    clerk,
    error,
    searchParams,
    signIn,
    t,
  ]);

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
      {error && binding && (
        <button type="button" onClick={() => window.location.reload()}>
          {t(($) => $.web.google_oauth.retry)}
        </button>
      )}
    </ClerkAuthShell>
  );
}
