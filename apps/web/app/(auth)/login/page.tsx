"use client";

import { Suspense, useMemo, useState } from "react";
import { SignIn, useAuth } from "@clerk/nextjs";
import { api } from "@patchbay/core/api";
import { useSearchParams } from "next/navigation";
import { redirectToCliCallback, validateCliCallback } from "@patchbay/views/auth";
import { ClerkAuthShell } from "@/components/clerk-auth-shell";

function resolveSafeRedirectUrl(raw: string | null): string {
  if (!raw) return "/";

  // The proxy emits an internal path. Keep the query/hash because they can
  // carry the original deep-link state, but never turn an arbitrary external
  // URL into a post-login redirect.
  if (raw.startsWith("/") && !raw.startsWith("//")) {
    const url = new URL(raw, "https://patchbay.invalid");
    return `${url.pathname}${url.search}${url.hash}` || "/";
  }

  if (typeof window === "undefined") return "/";
  try {
    const url = new URL(raw);
    if (url.origin !== window.location.origin) return "/";
    return `${url.pathname}${url.search}${url.hash}` || "/";
  } catch {
    return "/";
  }
}

export default function LoginPage() {
  return (
    <Suspense>
      <LoginContent />
    </Suspense>
  );
}

function LoginContent() {
  const searchParams = useSearchParams();
  const { isLoaded, isSignedIn } = useAuth();
  const [error, setError] = useState("");
  const cliCallback = searchParams.get("cli_callback") ?? "";
  const cliState = searchParams.get("cli_state") ?? "";
  const requestedRedirectUrl = searchParams.get("redirect_url");
  const validCliCallback = cliCallback !== "" && validateCliCallback(cliCallback);
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
        <p role="alert">Invalid CLI callback URL.</p>
      </ClerkAuthShell>
    );
  }

  if (validCliCallback && isLoaded && isSignedIn) {
    const authorize = async () => {
      setError("");
      try {
        // The managed web identity boundary authenticates the Clerk session
        // supplied by the ApiClient. The backend then exchanges that identity
        // for the native Patchbay bearer understood by the CLI and Rust API.
        const { token } = await api.issueCliToken();
        if (!token) throw new Error("Patchbay CLI token unavailable");
        redirectToCliCallback(cliCallback, token, cliState);
      } catch {
        setError("Could not authorize the CLI. Please try again.");
      }
    };

    return (
      <ClerkAuthShell>
        <div className="flex flex-col items-center gap-3">
          <p>Authorize Patchbay CLI for this signed-in account?</p>
          <button
            type="button"
            onClick={authorize}
            className="rounded bg-primary px-4 py-2 text-primary-foreground"
          >
            Authorize CLI
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
        signUpUrl="/signup"
        forceRedirectUrl={returnUrl}
      />
    </ClerkAuthShell>
  );
}
