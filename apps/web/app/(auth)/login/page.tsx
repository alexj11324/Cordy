"use client";

import { Suspense, useMemo, useState } from "react";
import { SignIn, useAuth } from "@clerk/nextjs";
import { api } from "@cordy/core/api";
import { useSearchParams } from "next/navigation";
import { redirectToCliCallback, validateCliCallback } from "@cordy/views/auth";

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
  const validCliCallback = cliCallback !== "" && validateCliCallback(cliCallback);
  const returnUrl = useMemo(() => {
    if (!validCliCallback) return "/";
    const params = new URLSearchParams({
      cli_callback: cliCallback,
      cli_state: cliState,
    });
    return `/login?${params.toString()}`;
  }, [cliCallback, cliState, validCliCallback]);

  if (cliCallback && !validCliCallback) {
    return (
      <div className="flex min-h-screen items-center justify-center bg-background">
        <p role="alert">Invalid CLI callback URL.</p>
      </div>
    );
  }

  if (validCliCallback && isLoaded && isSignedIn) {
    const authorize = async () => {
      setError("");
      try {
        // The managed web identity boundary authenticates the Clerk session
        // supplied by the ApiClient. The backend then exchanges that identity
        // for the native Cordy bearer understood by the CLI and Rust API.
        const { token } = await api.issueCliToken();
        if (!token) throw new Error("Cordy CLI token unavailable");
        redirectToCliCallback(cliCallback, token, cliState);
      } catch {
        setError("Could not authorize the CLI. Please try again.");
      }
    };

    return (
      <div className="flex min-h-screen items-center justify-center bg-background">
        <div className="flex flex-col items-center gap-3">
          <p>Authorize Cordy CLI for this signed-in account?</p>
          <button
            type="button"
            onClick={authorize}
            className="rounded bg-primary px-4 py-2 text-primary-foreground"
          >
            Authorize CLI
          </button>
          {error && <p role="alert">{error}</p>}
        </div>
      </div>
    );
  }

  return (
    <div className="flex min-h-screen items-center justify-center bg-background">
      <SignIn
        routing="path"
        path="/login"
        signUpUrl="/signup"
        forceRedirectUrl={returnUrl}
      />
    </div>
  );
}
