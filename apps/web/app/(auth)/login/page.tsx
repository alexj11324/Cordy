"use client";

import {
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { SignIn, useAuth } from "@clerk/nextjs";
import { useAuthStore } from "@patchbay/core/auth";
import { api } from "@patchbay/core/api";
import { useSearchParams } from "next/navigation";
import {
  redirectToCliCallback,
  redirectToDesktopApp,
  validateCliCallback,
} from "@patchbay/views/auth";
import { useT } from "@patchbay/views/i18n";
import { ClerkAuthShell } from "@/components/clerk-auth-shell";

function desktopHandoffQuery(codeChallenge: string, state: string): string {
  const params = new URLSearchParams({ platform: "desktop" });
  if (codeChallenge) params.set("code_challenge", codeChallenge);
  if (state) params.set("state", state);
  return params.toString();
}

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
  const patchbayAuthStatus = useAuthStore((state) => state.status);
  const [error, setError] = useState("");
  const cliCallback = searchParams.get("cli_callback") ?? "";
  const cliState = searchParams.get("cli_state") ?? "";
  const desktopHandoff = searchParams.get("platform") === "desktop";
  const desktopCodeChallenge = searchParams.get("code_challenge") ?? "";
  const desktopState = searchParams.get("state") ?? "";
  const requestedRedirectUrl = searchParams.get("redirect_url");
  const validCliCallback = cliCallback !== "" && validateCliCallback(cliCallback);
  const returnUrl = useMemo(() => {
    if (desktopHandoff) {
      return `/login?${desktopHandoffQuery(desktopCodeChallenge, desktopState)}`;
    }
    if (!validCliCallback) return resolveSafeRedirectUrl(requestedRedirectUrl);
    const params = new URLSearchParams({
      cli_callback: cliCallback,
      cli_state: cliState,
    });
    return `/login?${params.toString()}`;
  }, [
    cliCallback,
    cliState,
    desktopCodeChallenge,
    desktopHandoff,
    desktopState,
    requestedRedirectUrl,
    validCliCallback,
  ]);

  if (cliCallback && !validCliCallback) {
    return (
      <ClerkAuthShell>
        <p role="alert">Invalid CLI callback URL.</p>
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

  if (desktopHandoff && isLoaded && isSignedIn) {
    return (
      <DesktopHandoff
        codeChallenge={desktopCodeChallenge}
        state={desktopState}
      />
    );
  }

  return (
    <ClerkAuthShell>
      <SignIn
        routing="path"
        path="/login"
        signUpUrl={
          desktopHandoff
            ? `/signup?${desktopHandoffQuery(desktopCodeChallenge, desktopState)}`
            : "/signup"
        }
        forceRedirectUrl={returnUrl}
      />
    </ClerkAuthShell>
  );
}

function DesktopHandoff({
  codeChallenge,
  state,
}: {
  codeChallenge: string;
  state: string;
}) {
  const { t } = useT("auth");
  const authStatus = useAuthStore((state) => state.status);
  const backendSessionReady = authStatus === "authenticated";
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const automaticAttempted = useRef(false);

  const openDesktopApp = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      if (!codeChallenge || !state) {
        throw new Error("Patchbay desktop handoff is missing its binding");
      }
      const { code } = await api.issueDesktopHandoff(codeChallenge);
      if (!code) throw new Error("Patchbay desktop handoff code unavailable");
      redirectToDesktopApp(code, state);
      setLoading(false);
    } catch {
      setError(t(($) => $.web.desktop_handoff.prepare_failed));
      setLoading(false);
    }
  }, [codeChallenge, state, t]);

  useEffect(() => {
    if (!backendSessionReady || automaticAttempted.current) return;
    automaticAttempted.current = true;
    void openDesktopApp();
  }, [backendSessionReady, openDesktopApp]);

  return (
    <ClerkAuthShell>
      <div className="flex w-full max-w-sm flex-col items-center gap-4 rounded-2xl border border-border bg-card p-6 text-center shadow-sm">
        <h1 className="text-title-sm font-semibold">
          {t(($) => $.web.desktop_handoff.opening_title)}
        </h1>
        <p aria-live="polite" className="text-body text-muted-foreground">
          {!backendSessionReady || loading
            ? t(($) => $.web.desktop_handoff.preparing)
            : t(($) => $.web.desktop_handoff.opening_description)}
        </p>
        <button
          type="button"
          onClick={openDesktopApp}
          disabled={loading || !backendSessionReady}
          className="inline-flex min-h-10 items-center justify-center rounded-md bg-primary px-4 py-2 text-body font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:pointer-events-none disabled:opacity-60"
        >
          {loading || !backendSessionReady
            ? t(($) => $.web.desktop_handoff.preparing)
            : t(($) => $.web.desktop_handoff.open_button)}
        </button>
        {error && (
          <p role="alert" className="text-body text-destructive">
            {error}
          </p>
        )}
      </div>
    </ClerkAuthShell>
  );
}
