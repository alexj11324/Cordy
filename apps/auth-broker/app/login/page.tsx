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
  completeDesktopGoogleAttempt,
  registerDesktopGoogleAttempt,
} from "@/lib/broker-client";
import {
  buildDesktopCallbackUrl,
  desktopAttemptStorageKey,
  readDesktopHandoffBinding,
} from "@/lib/desktop-handoff";
import { useAuthMessages } from "@/lib/auth-messages";
import { resolveStandaloneReturnUrl } from "@/lib/redirect";

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
  const { isLoaded, isSignedIn, sessionId, getToken } = useAuth();
  const { signOut } = useClerk();
  const messages = useAuthMessages();
  const redirecting = useRef(false);
  const handoffAttempted = useRef(false);
  const registering = useRef(false);
  const retiringSession = useRef<string | null>(null);
  const [error, setError] = useState(false);
  const [restartRequired, setRestartRequired] = useState(false);
  const [prepared, setPrepared] = useState(false);
  const [callbackUrl, setCallbackUrl] = useState<string | null>(null);
  const [handoffStarted, setHandoffStarted] = useState(false);

  const returnUrl = useMemo(() => {
    if (binding) return `/login?${binding.query}`;
    return resolveStandaloneReturnUrl(
      params.get("return_url") ?? params.get("redirect_url"),
    );
  }, [binding, params]);
  const storageKey = binding
    ? desktopAttemptStorageKey(binding.state)
    : "";

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
    } catch {
      setRestartRequired(true);
      setHandoffStarted(false);
      setError(true);
    }
  }, [binding, getToken, storageKey]);

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

    // A returning authentication has a registered attempt in this tab. A new
    // Desktop request must prepare fresh authentication before rendering a form.
    if (!prepared) {
      if (window.sessionStorage.getItem(storageKey) === binding.codeChallenge) {
        setPrepared(true);
        return;
      }
      if (registering.current) return;
      registering.current = true;
      void (async () => {
        await registerDesktopGoogleAttempt({ state: binding.state, code_challenge: binding.codeChallenge });
        if (isSignedIn) {
          if (!sessionId) throw new Error("Active session unavailable");
          retiringSession.current = sessionId;
          await signOut(() => undefined, { sessionId });
        }
        window.sessionStorage.setItem(storageKey, binding.codeChallenge);
        setPrepared(true);
      })().catch(() => setError(true));
      return;
    }
    if (retiringSession.current && sessionId === retiringSession.current) return;
    if (isSignedIn && !handoffAttempted.current) {
      handoffAttempted.current = true;
      void complete();
    }
  }, [
    binding,
    complete,
    desktopRequest,
    isLoaded,
    isSignedIn,
    sessionId,
    prepared,
    returnUrl,
    signOut,
    storageKey,
  ]);

  if (error) {
    return (
      <AuthShell>
        <div className="accounts-auth-message">
          <p role="alert">{restartRequired ? messages.desktopRestart : messages.desktopFailed}</p>
          {!restartRequired && <button type="button" onClick={() => window.location.reload()}>
            {messages.retry}
          </button>}
        </div>
      </AuthShell>
    );
  }

  const awaitingSignOut = Boolean(retiringSession.current && sessionId === retiringSession.current);
  if (!isLoaded || (binding && (!prepared || awaitingSignOut))) {
    return <AuthShell><p role="status">{messages.preparing}</p></AuthShell>;
  }

  // Render from the authentication state itself: an effect must not briefly
  // expose the account form before it starts completing an authenticated return.
  const finishing = Boolean(callbackUrl) || handoffStarted || Boolean(isSignedIn);

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
