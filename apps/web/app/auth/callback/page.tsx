"use client";

import { Suspense, useEffect, useState } from "react";
import { useSearchParams } from "next/navigation";
import { paths } from "@patchbay/core/paths";
import { api } from "@patchbay/core/api";
import { validateCliCallback, redirectToCliCallback } from "@patchbay/views/auth";
import {
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
} from "@patchbay/ui/components/ui/card";
import { Button } from "@patchbay/ui/components/ui/button";
import { Loader2 } from "lucide-react";

function redirectToDesktopHandoff(code: string, state: string): void {
  const url = new URL("patchbay://auth/callback");
  url.searchParams.set("code", code);
  url.searchParams.set("state", state);
  window.location.href = url.href;
}

function decodeStateValue(value: string, prefix: string): string | null {
  try {
    return decodeURIComponent(value.slice(prefix.length));
  } catch {
    return null;
  }
}

function CallbackContent() {
  const searchParams = useSearchParams();
  const [error, setError] = useState("");
  const [desktopHandoff, setDesktopHandoff] = useState<{
    code: string;
    state: string;
  } | null>(null);

  useEffect(() => {
    const errorParam = searchParams.get("error");
    if (errorParam) {
      setError(errorParam === "access_denied" ? "Access denied" : errorParam);
      return;
    }

    const code = searchParams.get("code");
    if (!code) {
      setError("Missing authorization code");
      return;
    }

    const state = searchParams.get("state") || "";
    const stateParts = state.split(",");
    const isDesktop = stateParts.includes("platform:desktop");
    const desktopStatePart = stateParts.find((p) => p.startsWith("desktop_state:"));
    const desktopCodeChallengePart = stateParts.find((p) =>
      p.startsWith("desktop_code_challenge:"),
    );
    const desktopState = desktopStatePart
      ? decodeStateValue(desktopStatePart, "desktop_state:")
      : null;
    const desktopCodeChallenge = desktopCodeChallengePart
      ? decodeStateValue(
          desktopCodeChallengePart,
          "desktop_code_challenge:",
        )
      : null;
    // CLI callback params — carried across the Google OAuth round-trip so
    // headless/WSL2 `patchbay login` can receive the JWT after browser-based
    // Google auth completes.
    const cliCallbackPart = stateParts.find((p) => p.startsWith("cli_callback:"));
    const cliStatePart = stateParts.find((p) => p.startsWith("cli_state:"));
    const cliCallbackRaw = cliCallbackPart
      ? decodeStateValue(cliCallbackPart, "cli_callback:")
      : null;
    const cliState = cliStatePart
      ? decodeStateValue(cliStatePart, "cli_state:") ?? ""
      : "";

    const redirectUri = `${window.location.origin}/auth/callback`;

    // Validate the CLI callback URL before redirecting — the state parameter
    // passes through Google OAuth and must be treated as attacker-controlled.
    const cliCallback =
      cliCallbackRaw && validateCliCallback(cliCallbackRaw)
        ? cliCallbackRaw
        : null;

    if (cliCallback) {
      // CLI login flow: exchange the Google code for a JWT, then redirect the
      // token back to the CLI's local HTTP listener (e.g. WSL2 host).
      api
        .googleLogin(code, redirectUri)
        .then(({ token }) => {
          redirectToCliCallback(cliCallback, token, cliState);
        })
        .catch((err) => {
          setError(err instanceof Error ? err.message : "Login failed");
        });
    } else if (isDesktop) {
      // Desktop flow: the Google exchange sets the browser cookie, then the
      // authenticated session completes the registered PKCE handoff. Only a
      // short-lived one-time code crosses the custom protocol boundary.
      if (!desktopState || !desktopCodeChallenge) {
        setError("Invalid desktop auth handoff");
        return;
      }
      api
        .googleLogin(code, redirectUri)
        .then(async () => {
          const handoff = await api.completeDesktopAuthHandoff(
            desktopState,
            desktopCodeChallenge,
          );
          setDesktopHandoff({ code: handoff.code, state: handoff.state });
          redirectToDesktopHandoff(handoff.code, handoff.state);
        })
        .catch((err) => {
          setError(err instanceof Error ? err.message : "Login failed");
        });
    } else {
      // Google is a broker for Desktop and explicitly-authorized CLI flows.
      // Ordinary Web login uses the email send-code flow; accepting a bare
      // OAuth callback here would put the legacy /auth/google exchange back
      // on the Web main path and would also accept forged state-less links.
      setError("Unsupported login callback");
    }
  }, [searchParams]);

  if (desktopHandoff) {
    return (
      <div className="flex min-h-screen items-center justify-center">
        <Card className="w-full max-w-sm">
          <CardHeader className="text-center">
            <CardTitle className="text-display-sm">Opening Orvilo</CardTitle>
            <CardDescription>
              You should see a prompt to open the Orvilo desktop app. If
              nothing happens, click the button below.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex justify-center">
            <Button
              variant="outline"
              onClick={() => {
                redirectToDesktopHandoff(
                  desktopHandoff.code,
                  desktopHandoff.state,
                );
              }}
            >
              Open Orvilo Desktop
            </Button>
          </CardContent>
        </Card>
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex min-h-screen items-center justify-center">
        <Card className="w-full max-w-sm">
          <CardHeader className="text-center">
            <CardTitle className="text-display-sm">Login Failed</CardTitle>
            <CardDescription>{error}</CardDescription>
          </CardHeader>
          <CardContent className="flex justify-center">
            <a href={paths.login()} className="text-primary underline-offset-4 hover:underline">
              Back to login
            </a>
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div className="flex min-h-screen items-center justify-center">
      <Card className="w-full max-w-sm">
        <CardHeader className="text-center">
          <CardTitle className="text-display-sm">Signing in...</CardTitle>
          <CardDescription>Please wait while we complete your login</CardDescription>
        </CardHeader>
        <CardContent className="flex justify-center">
          <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
        </CardContent>
      </Card>
    </div>
  );
}

export default function CallbackPage() {
  return (
    <Suspense fallback={null}>
      <CallbackContent />
    </Suspense>
  );
}
