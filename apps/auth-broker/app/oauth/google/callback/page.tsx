"use client";

import { Suspense, useEffect, useMemo, useRef, useState } from "react";
import { useClerk, useSignIn, useSignUp } from "@clerk/nextjs";
import { useRouter, useSearchParams } from "next/navigation";
import { AuthShell } from "@/components/auth-shell";
import { useAuthMessages } from "@/lib/auth-messages";
import { readDesktopHandoffBinding } from "@/lib/desktop-handoff";
import {
  consumeGoogleOAuthNonce,
  googleOAuthAttemptIsReady,
} from "@/lib/google-oauth";

export default function GoogleOAuthCallbackPage() {
  return (
    <Suspense>
      <GoogleOAuthCallbackContent />
    </Suspense>
  );
}

function GoogleOAuthCallbackContent() {
  const searchParams = useSearchParams();
  const binding = useMemo(
    () => readDesktopHandoffBinding(searchParams),
    [searchParams],
  );
  const clerk = useClerk();
  const { signIn } = useSignIn();
  const { signUp } = useSignUp();
  const router = useRouter();
  const { messages } = useAuthMessages();
  const attempted = useRef(false);
  const nonceConsumed = useRef(false);
  const [error, setError] = useState(false);

  useEffect(() => {
    if (!binding) {
      setError(true);
      return;
    }
    if (!clerk.loaded || attempted.current || !signIn || !signUp) return;
    const destination = `/login?${binding.query}`;
    const failClosed = () => setError(true);
    const navigate = (url: string) => {
      if (/^https?:\/\//.test(url)) window.location.assign(url);
      else router.replace(url);
    };
    type FinalizeOptions = NonNullable<Parameters<typeof signIn.finalize>[0]>;
    const handleNavigate: NonNullable<FinalizeOptions["navigate"]> = async ({
      session,
      decorateUrl,
    }) => {
      if (session?.currentTask) return failClosed();
      navigate(decorateUrl(destination));
    };
    const finalizeSignIn = async () => {
      const { error: finalizeError } = await signIn.finalize({
        navigate: handleNavigate,
      });
      if (finalizeError) failClosed();
    };
    const finalizeSignUp = async () => {
      const { error: finalizeError } = await signUp.finalize({
        navigate: handleNavigate,
      });
      if (finalizeError) failClosed();
    };

    const complete = async () => {
      if (signIn.status === "complete") return finalizeSignIn();
      if (signIn.isTransferable) {
        const { error: transferError } = await signUp.create({ transfer: true });
        if (transferError || (signUp.status as string) !== "complete") {
          return failClosed();
        }
        return finalizeSignUp();
      }
      if (signUp.isTransferable) {
        const { error: transferError } = await signIn.create({ transfer: true });
        if (transferError || (signIn.status as string) !== "complete") {
          return failClosed();
        }
        return finalizeSignIn();
      }
      if ((signUp.status as string) === "complete") return finalizeSignUp();
      const existingSessionId =
        signIn.existingSession?.sessionId ?? signUp.existingSession?.sessionId;
      if (!existingSessionId) return failClosed();
      await clerk.setActive({
        session: existingSessionId,
        navigate: async ({ session, decorateUrl }) => {
          if (session?.currentTask) return failClosed();
          navigate(decorateUrl(destination));
        },
      });
    };

    const run = async () => {
      if (!nonceConsumed.current) {
        const ready = await consumeGoogleOAuthNonce(
          signIn,
          searchParams.get("rotating_token_nonce"),
        );
        if (!ready) return;
        nonceConsumed.current = true;
      }
      if (!googleOAuthAttemptIsReady(signIn, signUp)) return;
      attempted.current = true;
      await complete();
    };
    void run().catch(failClosed);
  }, [binding, clerk, router, searchParams, signIn, signUp]);

  return (
    <AuthShell>
      <p role={error ? "alert" : "status"} aria-live="polite">
        {error
          ? binding
            ? messages.web.google_oauth.failed
            : messages.web.google_oauth.invalid_binding
          : messages.web.google_oauth.completing}
      </p>
      <div id="clerk-captcha" />
    </AuthShell>
  );
}
