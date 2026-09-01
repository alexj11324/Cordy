"use client";

import {
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useAuth } from "@clerk/nextjs";
import { useSearchParams } from "next/navigation";
import { AuthShell } from "@/components/auth-shell";
import { useAuthMessages } from "@/lib/auth-messages";
import {
  BrokerApiError,
  completeDesktopGoogleAttempt,
} from "@/lib/broker-client";
import {
  buildDesktopCallbackUrl,
  readDesktopHandoffBinding,
} from "@/lib/desktop-handoff";

export default function DesktopCompletionPage() {
  return (
    <Suspense>
      <DesktopCompletionContent />
    </Suspense>
  );
}

function DesktopCompletionContent() {
  const searchParams = useSearchParams();
  const binding = useMemo(
    () => readDesktopHandoffBinding(searchParams),
    [searchParams],
  );
  const { isLoaded, isSignedIn, getToken } = useAuth();
  const { messages } = useAuthMessages();
  const automaticAttempted = useRef(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(false);

  const openDesktopApp = useCallback(async () => {
    if (!binding) {
      setError(true);
      setLoading(false);
      return;
    }
    setLoading(true);
    setError(false);
    try {
      const sessionToken = await getToken();
      if (!sessionToken) throw new BrokerApiError(401);
      const { callbackProtocol, code } = await completeDesktopGoogleAttempt(
        sessionToken,
        {
          state: binding.state,
          code_challenge: binding.codeChallenge,
        },
      );
      window.location.assign(
        buildDesktopCallbackUrl(code, binding.state, callbackProtocol),
      );
      setLoading(false);
    } catch (caught) {
      if (
        caught instanceof BrokerApiError &&
        (caught.status === 401 || caught.status === 409)
      ) {
        window.location.replace(`/oauth/google?${binding.query}`);
        return;
      }
      setError(true);
      setLoading(false);
    }
  }, [binding, getToken]);

  useEffect(() => {
    if (!binding) {
      setError(true);
      setLoading(false);
      return;
    }
    if (!isLoaded || automaticAttempted.current) return;
    if (!isSignedIn) {
      automaticAttempted.current = true;
      window.location.replace(`/oauth/google?${binding.query}`);
      return;
    }
    automaticAttempted.current = true;
    void openDesktopApp();
  }, [binding, isLoaded, isSignedIn, openDesktopApp]);

  return (
    <AuthShell>
      <p role={error ? "alert" : "status"} aria-live="polite">
        {!binding
          ? messages.web.google_oauth.invalid_binding
          : error
            ? messages.web.desktop_handoff.prepare_failed
            : loading
              ? messages.web.desktop_handoff.preparing
              : messages.web.desktop_handoff.opening_description}
      </p>
      {binding && (
        <button
          type="button"
          disabled={loading}
          onClick={() => void openDesktopApp()}
        >
          {messages.web.desktop_handoff.open_button}
        </button>
      )}
    </AuthShell>
  );
}
