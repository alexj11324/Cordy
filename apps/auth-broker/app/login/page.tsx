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
  const { isLoaded, isSignedIn, getToken } = useAuth();
  const { signOut } = useClerk();
  const messages = useAuthMessages();
  const attempted = useRef(false);
  const [registered, setRegistered] = useState(false);
  const [error, setError] = useState(false);

  const returnUrl = binding ? `/login?${binding.query}` : "/login";
  const storageKey = binding
    ? `${ATTEMPT_STORAGE_PREFIX}${binding.state}`
    : ATTEMPT_STORAGE_PREFIX;

  const complete = useCallback(async () => {
    if (!binding) {
      setError(true);
      return;
    }
    try {
      const token = await getToken();
      if (!token) throw new Error("Clerk session token unavailable");
      const result = await completeDesktopGoogleAttempt(token, {
        state: binding.state,
        code_challenge: binding.codeChallenge,
      });
      window.sessionStorage.removeItem(storageKey);
      window.location.assign(
        buildDesktopCallbackUrl(
          result.code,
          binding.state,
          result.callbackProtocol,
        ),
      );
    } catch (failure) {
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
    if (!binding) {
      setError(true);
      return;
    }
    if (!isLoaded || attempted.current) return;

    if (isSignedIn) {
      attempted.current = true;
      void complete();
      return;
    }

    if (window.sessionStorage.getItem(storageKey) === binding.codeChallenge) {
      setRegistered(true);
      return;
    }

    attempted.current = true;
    void registerDesktopGoogleAttempt({
      state: binding.state,
      code_challenge: binding.codeChallenge,
    })
      .then(() => {
        window.sessionStorage.setItem(storageKey, binding.codeChallenge);
        setRegistered(true);
      })
      .catch(() => setError(true))
      .finally(() => {
        attempted.current = false;
      });
  }, [binding, complete, isLoaded, isSignedIn, storageKey]);

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

  if (!binding || !isLoaded || isSignedIn || !registered) {
    return (
      <AuthShell>
        <p role="status">{messages.opening}</p>
      </AuthShell>
    );
  }

  return (
    <AuthShell>
      <AccountsLoginForm returnUrl={returnUrl} />
    </AuthShell>
  );
}
