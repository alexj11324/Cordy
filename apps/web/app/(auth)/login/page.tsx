"use client";

import { Suspense, useMemo, useState } from "react";
import { SignIn, useAuth } from "@clerk/nextjs";
import { useAuthStore } from "@patchbay/core/auth";
import { api } from "@patchbay/core/api";
import {
  redirectToCliCallback,
  validateCliCallback,
} from "@patchbay/views/auth";
import { useT } from "@patchbay/views/i18n";
import { ClerkAuthShell } from "@/components/clerk-auth-shell";
import {
  authRouteWithRedirect,
  resolveSafeRedirectUrl,
} from "@/features/auth/safe-redirect";
import { useWebSearchParams } from "@/platform/client-navigation";

export default function LoginPage() {
  return (
    <Suspense>
      <LoginContent />
    </Suspense>
  );
}

function LoginContent() {
  const searchParams = useWebSearchParams();
  const { isLoaded, isSignedIn } = useAuth();
  const { t } = useT("auth");
  const patchbayAuthStatus = useAuthStore((state) => state.status);
  const [error, setError] = useState("");
  const cliCallback = searchParams.get("cli_callback") ?? "";
  const cliState = searchParams.get("cli_state") ?? "";
  const requestedRedirectUrl = searchParams.get("redirect_url");
  const validCliCallback =
    cliCallback !== "" && validateCliCallback(cliCallback);
  const returnUrl = useMemo(() => {
    if (!validCliCallback) return resolveSafeRedirectUrl(requestedRedirectUrl);
    const params = new URLSearchParams({
      cli_callback: cliCallback,
      cli_state: cliState,
    });
    return `/login?${params.toString()}`;
  }, [cliCallback, cliState, requestedRedirectUrl, validCliCallback]);

  if (cliCallback && !validCliCallback) {
    return (
      <ClerkAuthShell>
        <p role="alert">{t(($) => $.web.cli_authorization.invalid_callback)}</p>
      </ClerkAuthShell>
    );
  }

  if (
    validCliCallback &&
    isLoaded &&
    isSignedIn &&
    patchbayAuthStatus === "authenticated"
  ) {
    const authorize = async () => {
      setError("");
      try {
        // The managed web identity boundary authenticates the Clerk session
        // supplied by the ApiClient. The backend then exchanges that identity
        // for the native Patchbay bearer understood by the CLI and Go API.
        const { token } = await api.issueCliToken();
        if (!token) throw new Error("Patchbay CLI token unavailable");
        redirectToCliCallback(cliCallback, token, cliState);
      } catch {
        setError(t(($) => $.web.cli_authorization.failed));
      }
    };

    return (
      <ClerkAuthShell>
        <div className="flex flex-col items-center gap-3">
          <p>{t(($) => $.web.cli_authorization.prompt)}</p>
          <button
            type="button"
            onClick={authorize}
            className="rounded bg-primary px-4 py-2 text-primary-foreground"
          >
            {t(($) => $.web.cli_authorization.authorize_button)}
          </button>
          {error && <p role="alert">{error}</p>}
        </div>
      </ClerkAuthShell>
    );
  }

  return (
    <ClerkAuthShell>
      <SignIn
        routing="path"
        path="/login"
        signUpUrl={authRouteWithRedirect("/signup", returnUrl)}
        forceRedirectUrl={returnUrl}
      />
    </ClerkAuthShell>
  );
}
