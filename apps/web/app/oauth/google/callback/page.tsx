"use client";

import { Suspense, useEffect, useMemo, useRef, useState } from "react";
import { useClerk, useSignIn, useSignUp } from "@clerk/nextjs";
import { ClerkAuthShell } from "@/components/clerk-auth-shell";
import { buildBrokerRoute } from "@/features/auth/broker-path";
import { readDesktopHandoffBinding } from "@/features/auth/desktop-handoff";
import { useT } from "@patchbay/views/i18n";
import {
  useWebRouter,
  useWebSearchParams,
} from "@/platform/client-navigation";

export default function GoogleOAuthCallbackPage() {
  return (
    <Suspense>
      <GoogleOAuthCallbackContent />
    </Suspense>
  );
}

function GoogleOAuthCallbackContent() {
  const searchParams = useWebSearchParams();
  const binding = useMemo(
    () =>
      readDesktopHandoffBinding(searchParams),
    [searchParams],
  );
  const clerk = useClerk();
  const { signIn } = useSignIn();
  const { signUp } = useSignUp();
  const router = useWebRouter();
  const { t } = useT("auth");
  const attempted = useRef(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!binding) {
      setError(t(($) => $.web.google_oauth.invalid_binding));
      return;
    }
    if (!clerk.loaded || attempted.current) return;
    attempted.current = true;

    const destination = `${buildBrokerRoute(
      window.location.pathname,
      "/oauth/google/callback",
      "/login",
    )}?${binding.query}`;
    const failClosed = () => {
      setError(t(($) => $.web.google_oauth.failed));
    };
    const navigate = (url: string) => {
      if (/^https?:\/\//.test(url)) {
        window.location.assign(url);
      } else {
        router.replace(url);
      }
    };
    type FinalizeOptions = NonNullable<
      Parameters<typeof signIn.finalize>[0]
    >;
    const handleNavigate: NonNullable<FinalizeOptions["navigate"]> = async ({
      session,
      decorateUrl,
    }) => {
      if (session?.currentTask) {
        failClosed();
        return;
      }
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
      if (signIn.status === "complete") {
        await finalizeSignIn();
        return;
      }

      // A Google identity that does not belong to an existing Clerk user is
      // reported on the sign-in attempt as transferable. Continue that exact
      // verified external account as sign-up before considering the inverse
      // compatibility transfer.
      if (signIn.isTransferable) {
        const { error: transferError } = await signUp.create({
          transfer: true,
        });
        if (transferError) return failClosed();
        if ((signUp.status as string) === "complete") {
          await finalizeSignUp();
          return;
        }
        return failClosed();
      }

      if (signUp.isTransferable) {
        const { error: transferError } = await signIn.create({ transfer: true });
        if (transferError) return failClosed();
        if ((signIn.status as string) === "complete") {
          await finalizeSignIn();
          return;
        }
        return failClosed();
      }
      if ((signUp.status as string) === "complete") {
        await finalizeSignUp();
        return;
      }

      const existingSessionId =
        signIn.existingSession?.sessionId ?? signUp.existingSession?.sessionId;
      if (existingSessionId) {
        await clerk.setActive({
          session: existingSessionId,
          navigate: async ({ session, decorateUrl }) => {
            if (session?.currentTask) return failClosed();
            navigate(decorateUrl(destination));
          },
        });
        return;
      }

      // MFA, required profile fields, or any other unresolved Clerk task must
      // not fall through to a provider-selection card or mint a desktop code.
      failClosed();
    };

    void complete().catch(failClosed);
  }, [binding, clerk, router, signIn, signUp, t]);

  return (
    <ClerkAuthShell>
      <div className="flex flex-col items-center gap-4 text-center">
        <p
          role={error ? "alert" : "status"}
          aria-live="polite"
          className={
            error
              ? "text-body text-destructive"
              : "text-body text-muted-foreground"
          }
        >
          {error ?? t(($) => $.web.google_oauth.completing)}
        </p>
        <div id="clerk-captcha" />
      </div>
    </ClerkAuthShell>
  );
}
