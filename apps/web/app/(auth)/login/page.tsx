"use client";

import { Suspense, useCallback, useEffect, useRef, useState } from "react";
import { useSearchParams, useRouter } from "next/navigation";
import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { sanitizeNextUrl, useAuthStore } from "@patchbay/core/auth";
import { useConfigStore } from "@patchbay/core/config";
import {
  workspaceKeys,
  workspaceListOptions,
} from "@patchbay/core/workspace/queries";
import {
  paths,
  resolvePostAuthDestination,
  useHasOnboarded,
} from "@patchbay/core/paths";
import { api } from "@patchbay/core/api";
import type { Workspace } from "@patchbay/core/types";
import {
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
} from "@patchbay/ui/components/ui/card";
import { Button } from "@patchbay/ui/components/ui/button";
import { Loader2 } from "lucide-react";
import { setLoggedInCookie } from "@/features/auth/auth-cookie";
import Link from "next/link";
import { LoginPage, validateCliCallback } from "@patchbay/views/auth";
import { useT } from "@patchbay/views/i18n";

/**
 * Pick where a logged-in user with no explicit `?next=` should land.
 * Un-onboarded users with pending invitations on their email get routed to
 * the batch /invitations page; everyone else falls through to the standard
 * resolver. A network blip on listMyInvitations is non-fatal — we fall
 * through rather than trap the user on an error screen.
 */
async function resolveLoggedInDestination(
  qc: QueryClient,
  hasOnboarded: boolean,
  workspaces: Workspace[],
): Promise<string> {
  if (!hasOnboarded) {
    try {
      const invites = await api.listMyInvitations();
      if (invites.length > 0) {
        qc.setQueryData(workspaceKeys.myInvitations(), invites);
        return paths.invitations();
      }
    } catch {
      // fall through
    }
  }
  return resolvePostAuthDestination(workspaces, hasOnboarded);
}

function redirectToDesktopHandoff(code: string, state: string): void {
  const url = new URL("patchbay://auth/callback");
  url.searchParams.set("code", code);
  url.searchParams.set("state", state);
  window.location.href = url.href;
}

function LoginPageContent() {
  const router = useRouter();
  const qc = useQueryClient();
  const { t } = useT("auth");
  const googleClientId = useConfigStore((state) => state.googleClientId);
  const user = useAuthStore((s) => s.user);
  const isLoading = useAuthStore((s) => s.isLoading);
  const searchParams = useSearchParams();

  const cliCallbackRaw = searchParams.get("cli_callback");
  const cliState = searchParams.get("cli_state") || "";
  const platform = searchParams.get("platform");
  const isDesktopHandoff = platform === "desktop" && !cliCallbackRaw;
  const validatedCliCallback =
    cliCallbackRaw && validateCliCallback(cliCallbackRaw)
      ? cliCallbackRaw
      : null;
  const isGoogleBrokerFlow =
    isDesktopHandoff || validatedCliCallback !== null;
  const desktopState = searchParams.get("state") || "";
  const desktopCodeChallenge = searchParams.get("code_challenge") || "";
  // `next` carries a protected URL the user was originally headed to
  // (e.g. /invite/{id}). With URL-driven workspaces there is no legacy
  // "/issues" default — if `next` is absent we decide after login based on
  // the user's workspace list. Sanitize first so a crafted `?next=https://evil`
  // cannot bounce the user off-origin after a successful login.
  const nextUrl = sanitizeNextUrl(searchParams.get("next"));

  const [desktopHandoff, setDesktopHandoff] = useState<{
    code: string;
    state: string;
  } | null>(null);
  const [desktopError, setDesktopError] = useState("");
  const hasOnboarded = useHasOnboarded();

  // Latched once auth has been observed settled as logged-out on this page.
  // Any `user` that appears afterwards came from the login form in this
  // session — not from an existing session found on arrival.
  const settledLoggedOutRef = useRef(false);
  const desktopAttemptedRef = useRef(false);

  const completeDesktopHandoff = useCallback(async () => {
    if (!desktopState || !desktopCodeChallenge) {
      setDesktopError(t(($) => $.web.desktop_handoff.prepare_failed));
      return;
    }
    try {
      const handoff = await api.completeDesktopAuthHandoff(
        desktopState,
        desktopCodeChallenge,
      );
      setDesktopHandoff({ code: handoff.code, state: handoff.state });
      redirectToDesktopHandoff(handoff.code, handoff.state);
    } catch (err) {
      setDesktopError(
        err instanceof Error
          ? err.message
          : t(($) => $.web.desktop_handoff.prepare_failed),
      );
    }
  }, [desktopCodeChallenge, desktopState, t]);

  // Already authenticated ON ARRIVAL — honor ?next= or fall back to first
  // workspace (or /onboarding if the user has none). Skip this entire path
  // when the user arrived to authorize the CLI.
  useEffect(() => {
    if (isLoading) return;
    if (!user) {
      settledLoggedOutRef.current = true;
      return;
    }
    if (cliCallbackRaw) return;
    if (isDesktopHandoff) {
      // Desktop opened the browser for login but the web session is already
      // authenticated. Complete the registered PKCE handoff; only the
      // resulting one-time code crosses the custom protocol boundary.
      if (desktopAttemptedRef.current) return;
      desktopAttemptedRef.current = true;
      void completeDesktopHandoff();
      return;
    }
    // Fresh form login (issue #5009): `user` was written by verifyCode while
    // handleVerify was still fetching the workspace list, so this effect used
    // to read the not-yet-seeded list cache and race handleSuccess with a
    // replace to /workspaces/new. handleSuccess owns post-login navigation;
    // this effect only serves visitors who arrived already authenticated.
    if (settledLoggedOutRef.current) return;
    if (nextUrl) {
      router.replace(nextUrl);
      return;
    }
    // Fetch instead of reading the cache: on a fresh page load the cache is
    // cold, and `getQueryData() ?? []` would misroute a user who does have
    // workspaces to /workspaces/new. On fetch failure fall back to [] —
    // same destination the cold-cache read produced, rather than trapping
    // the user on the login page.
    void qc
      .ensureQueryData(workspaceListOptions())
      .catch(() => [] as Workspace[])
      .then((list) => resolveLoggedInDestination(qc, hasOnboarded, list))
      .then((dest) => router.replace(dest));
  }, [
    isLoading,
    user,
    router,
    nextUrl,
    cliCallbackRaw,
    isDesktopHandoff,
    hasOnboarded,
    qc,
    completeDesktopHandoff,
  ]);

  const handleSuccess = async () => {
    if (isDesktopHandoff) {
      await completeDesktopHandoff();
      return;
    }
    // Read the latest user snapshot directly — the closure's `hasOnboarded`
    // was captured before login completed and would be stale here.
    const currentUser = useAuthStore.getState().user;
    const onboarded = currentUser?.onboarded_at != null;
    if (nextUrl) {
      router.push(nextUrl);
      return;
    }
    const list = qc.getQueryData<Workspace[]>(workspaceKeys.list()) ?? [];
    router.push(await resolveLoggedInDestination(qc, onboarded, list));
  };

  // Build Google OAuth state: encode platform, desktop PKCE binding, next URL,
  // and CLI callback
  // params so the callback can redirect to the right place after login.
  // CLI callback/state must survive the Google OAuth round-trip so the
  // post-login callback page can redirect the JWT back to the CLI's local
  // HTTP listener (critical for headless / WSL2 environments).
  const googleState = [
    platform === "desktop" ? "platform:desktop" : "",
    isDesktopHandoff && desktopState
      ? `desktop_state:${encodeURIComponent(desktopState)}`
      : "",
    isDesktopHandoff && desktopCodeChallenge
      ? `desktop_code_challenge:${encodeURIComponent(desktopCodeChallenge)}`
      : "",
    nextUrl ? `next:${nextUrl}` : "",
    validatedCliCallback
      ? `cli_callback:${encodeURIComponent(validatedCliCallback)}`
      : "",
    cliState ? `cli_state:${encodeURIComponent(cliState)}` : "",
  ]
    .filter(Boolean)
    .join(",") || undefined;

  // While the desktop handoff is in progress (or has produced a code/error),
  // render a dedicated screen instead of flashing the login form or redirecting
  // away to a workspace page.
  if (isDesktopHandoff && user) {
    if (desktopError) {
      return (
        <div className="flex min-h-screen items-center justify-center">
          <Card className="w-full max-w-sm">
            <CardHeader className="text-center">
              <CardTitle className="text-display-sm">
                {t(($) => $.web.desktop_handoff.failed_title)}
              </CardTitle>
              <CardDescription>{desktopError}</CardDescription>
            </CardHeader>
          </Card>
        </div>
      );
    }
    return (
      <div className="flex min-h-screen items-center justify-center">
        <Card className="w-full max-w-sm">
          <CardHeader className="text-center">
            <CardTitle className="text-display-sm">
              {t(($) => $.web.desktop_handoff.opening_title)}
            </CardTitle>
            <CardDescription>
              {desktopHandoff
                ? t(($) => $.web.desktop_handoff.opening_description)
                : t(($) => $.web.desktop_handoff.preparing)}
            </CardDescription>
          </CardHeader>
          <CardContent className="flex justify-center">
            {desktopHandoff ? (
              <Button
                variant="outline"
                onClick={() => {
                  redirectToDesktopHandoff(
                    desktopHandoff.code,
                    desktopHandoff.state,
                  );
                }}
              >
                {t(($) => $.web.desktop_handoff.open_button)}
              </Button>
            ) : (
              <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
            )}
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <LoginPage
      onSuccess={handleSuccess}
      google={
        googleClientId && isGoogleBrokerFlow
          ? {
              clientId: googleClientId,
              redirectUri: `${window.location.origin}/auth/callback`,
              state: googleState,
            }
          : undefined
      }
      cliCallback={
        validatedCliCallback
          ? { url: validatedCliCallback, state: cliState }
          : undefined
      }
      onTokenObtained={setLoggedInCookie}
      extra={
        <span className="text-caption text-muted-foreground">
          {t(($) => $.web.prefer_desktop)}{" "}
          <Link
            href="/download"
            className="font-medium text-foreground underline decoration-foreground/30 underline-offset-4 hover:decoration-foreground/70"
          >
            {t(($) => $.web.download)}
          </Link>
        </span>
      }
    />
  );
}

export default function Page() {
  return (
    <Suspense fallback={null}>
      <LoginPageContent />
    </Suspense>
  );
}
