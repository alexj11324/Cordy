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
import { Loader2 } from "lucide-react";

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
    } else {
      // Desktop login completes on Accounts, never on the product web origin.
      // Ordinary Web login uses the email send-code flow; accepting a bare
      // OAuth callback here would put the legacy /auth/google exchange back
      // on the Web main path and would also accept forged state-less links.
      setError("Unsupported login callback");
    }
  }, [searchParams]);

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
