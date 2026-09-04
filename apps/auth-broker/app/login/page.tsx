"use client";

import {
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useAuth, useClerk } from "@clerk/nextjs";
import { useSearchParams } from "next/navigation";
import { AuthShell } from "@/components/auth-shell";
import { AccountsLoginForm } from "@/components/accounts-login-form";
import {
  BrokerApiError,
  completeDesktopGoogleAttempt,
  registerDesktopGoogleAttempt,
} from "@/lib/broker-client";
import {
  buildDesktopCallbackUrl,
  readDesktopHandoffBinding,
} from "@/lib/desktop-handoff";
import { useAuthMessages } from "@/lib/auth-messages";
import { resolveStandaloneReturnUrl } from "@/lib/redirect";

const ATTEMPT_STORAGE_PREFIX = "patchbay_desktop_attempt:";

export default function Page() {
  return (
    <Suspense>
      <Content />
    </Suspense>
  );
}

function Content() {
  const params = useSearchParams();
  const binding = useMemo(() => readDesktopHandoffBinding(params), [params]);
  const desktopRequest = params.get("platform") === "desktop";
  const { isLoaded, isSignedIn, getToken } = useAuth();
  const { signOut } = useClerk();
  const messages = useAuthMessages();
  const redirecting = useRef(false);
  const handoffAttempted = useRef(false);
  const registering = useRef(false);
  const [error, setError] = useState(false);
  const [callbackUrl, setCallbackUrl] = useState<string | null>(null);
  const [handoffStarted, setHandoffStarted] = useState(false);

  const returnUrl = useMemo(() => {
    if (binding) return `/login?${binding.query}`;
    return resolveStandaloneReturnUrl(
      params.get("return_url") ?? params.get("redirect_url"),
    );
  }, [binding, params]);
  const storageKey = binding
    ? `${ATTEMPT_STORAGE_PREFIX}${binding.state}`
    : ATTEMPT_STORAGE_PREFIX;

  const complete = useCallback(async () => {
    if (!binding) {
      setError(true);
      return;
    }
    setHandoffStarted(true);
    try {
      const token = await getToken();
      if (!token) throw new Error("Clerk session token unavailable");
      const result = await completeDesktopGoogleAttempt(token, {
        state: binding.state,
        code_challenge: binding.codeChallenge,
        ...(binding.local ? { local: true } : {}),
      });
      window.sessionStorage.removeItem(storageKey);
      setCallbackUrl(
        buildDesktopCallbackUrl(
          result.code,
          binding.state,
          result.callbackProtocol,
        ),
      );
    } catch (failure) {
      setHandoffStarted(false);
      if (
        failure instanceof BrokerApiError &&
        (failure.status === 401 || failure.status === 409)
      ) {
        window.sessionStorage.removeItem(storageKey);
        await signOut({ redirectUrl: returnUrl }).catch(() => setError(true));
        return;
      }
      setError(true);
    }
  }, [binding, getToken, returnUrl, signOut, storageKey]);

  useEffect(() => {
    if (!callbackUrl) return;
    try {
      window.location.assign(callbackUrl);
    } catch {
      // Browsers may ignore custom-protocol navigation; jsdom may throw.
      // The visible open link remains the user-gesture fallback.
    }
  }, [callbackUrl]);

  useEffect(() => {
    if (desktopRequest && !binding) {
      setError(true);
      return;
    }
    if (!isLoaded) return;

    if (!binding) {
      if (isSignedIn && !redirecting.current) {
        redirecting.current = true;
        window.location.assign(returnUrl);
      }
      return;
    }

    if (isSignedIn) {
      if (handoffAttempted.current) return;
      handoffAttempted.current = true;
      void complete();
      return;
    }

    if (window.sessionStorage.getItem(storageKey) === binding.codeChallenge) {
      return;
    }
    if (registering.current) return;
    registering.current = true;
    void registerDesktopGoogleAttempt({
      state: binding.state,
      code_challenge: binding.codeChallenge,
    })
      .then(() => {
        window.sessionStorage.setItem(storageKey, binding.codeChallenge);
      })
      .catch(() => setError(true))
      .finally(() => {
        registering.current = false;
      });
  }, [
    binding,
    complete,
    desktopRequest,
    isLoaded,
    isSignedIn,
    returnUrl,
    signOut,
    storageKey,
  ]);

  if (error) {
    return (
      <AuthShell>
        <div className="accounts-auth-message">
          <p role="alert">{messages.desktopFailed}</p>
          <button type="button" onClick={() => window.location.reload()}>
            {messages.retry}
          </button>
        </div>
      </AuthShell>
    );
  }

  const finishing = Boolean(callbackUrl) || handoffStarted;

  if (finishing) {
    return (
      <AuthShell>
        <div className="accounts-auth-message">
          <p role="status">{messages.finishing}</p>
          {callbackUrl && (
            <a href={callbackUrl}>{messages.open}</a>
          )}

        </div>
      </AuthShell>
    );
  }

  return (
    <AuthShell>
      <AccountsLoginForm returnUrl={returnUrl} />
    </AuthShell>
  );
}
