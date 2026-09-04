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
import {
  desktopSessionCompleteUrl,
  loopbackFreshKey,
} from "@/lib/session-api";
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
  const attempted = useRef(false);
  const redirecting = useRef(false);
  const [registered, setRegistered] = useState(false);
  const [error, setError] = useState(false);

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
    try {
      const token = await getToken();
      if (!token) throw new Error("Clerk session token unavailable");
      if (binding.sessionApi) {
        window.sessionStorage.removeItem(loopbackFreshKey(binding.state));
        submitLoopbackDesktopSession(binding.sessionApi, {
          session: token,
          state: binding.state,
          code_challenge: binding.codeChallenge,
        });
        return;
      }
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
    if (desktopRequest && !binding) {
      setError(true);
      return;
    }
    if (!isLoaded || attempted.current) return;

    if (!binding) {
      if (isSignedIn && !redirecting.current) {
        redirecting.current = true;
        window.location.assign(returnUrl);
      }
      return;
    }

    if (binding.sessionApi) {
      setRegistered(true);
      const freshKey = loopbackFreshKey(binding.state);
      if (isSignedIn && window.sessionStorage.getItem(freshKey) === "1") {
        attempted.current = true;
        void complete();
        return;
      }
      if (isSignedIn) {
        attempted.current = true;
        void signOut({ redirectUrl: returnUrl }).catch(() => setError(true));
        return;
      }
      window.sessionStorage.setItem(freshKey, "1");
      return;
    }

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

  if (!isLoaded || isSignedIn || (binding && !registered)) {
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

function submitLoopbackDesktopSession(
  sessionApi: string,
  fields: { session: string; state: string; code_challenge: string },
): void {
  const form = document.createElement("form");
  form.method = "POST";
  form.action = desktopSessionCompleteUrl(sessionApi);
  form.acceptCharset = "UTF-8";
  for (const [name, value] of Object.entries(fields)) {
    const input = document.createElement("input");
    input.type = "hidden";
    input.name = name;
    input.value = value;
    form.appendChild(input);
  }
  document.body.appendChild(form);
  form.submit();
}
