"use client";

import { useState, useEffect, useCallback, useRef, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
  CardFooter,
} from "@patchbay/ui/components/ui/card";
import { Input } from "@patchbay/ui/components/ui/input";
import { Button } from "@patchbay/ui/components/ui/button";
import { Label } from "@patchbay/ui/components/ui/label";
import {
  InputOTP,
  InputOTPGroup,
  InputOTPSlot,
} from "@patchbay/ui/components/ui/input-otp";
import {
  Field,
  FieldError,
  FieldGroup,
  FieldLabel,
  FieldSeparator,
} from "@patchbay/ui/components/ui/field";
import { useAuthStore } from "@patchbay/core/auth";
import { workspaceKeys } from "@patchbay/core/workspace/queries";
import { api } from "@patchbay/core/api";
import type { User } from "@patchbay/core/types";
import { useT } from "../i18n";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface CliCallbackConfig {
  /** Validated localhost callback URL */
  url: string;
  /** Opaque state to pass back to CLI */
  state: string;
}

interface LoginPageProps {
  /** Logo element rendered above the title */
  logo?: ReactNode;
  /** Called after successful login. The workspace list is seeded into React
   *  Query before this fires, so the caller can compute a destination URL. */
  onSuccess: () => void;
  /** CLI callback config for authorizing CLI tools. */
  cliCallback?: CliCallbackConfig;
  /** Called after a token is obtained (e.g. to set cookies). */
  onTokenObtained?: () => void;
  /** Canonical brokered Google login handler (e.g. desktop opens browser externally). */
  onGoogleLogin?: () => void;
  /** Render the cardless narrow form used by the authentication example. */
  embedded?: boolean;
  /** Render the example's separator between Email and Google. */
  showGoogleSeparator?: boolean;
  /** Disable Google while an external desktop login is opening. */
  googleLoading?: boolean;
  /** Error state owned by the embedding shell, rendered in the same form column. */
  externalError?: ReactNode;
  /** Slot rendered at the bottom of the sign-in card, below the
   *  Google button. The web shell uses it for a "Prefer the desktop
   *  app?" prompt; desktop omits it (a download prompt inside the app
   *  would be absurd). */
  extra?: ReactNode;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

export function redirectToCliCallback(url: string, token: string, state: string) {
  const separator = url.includes("?") ? "&" : "?";
  window.location.href = `${url}${separator}token=${encodeURIComponent(token)}&state=${encodeURIComponent(state)}`;
}

/**
 * Hand a freshly issued native session to the installed desktop app. The
 * custom protocol lets the OS show its normal "Open Patchbay?" confirmation
 * before Electron receives the one-time code through its deep-link handler.
 */
export function redirectToDesktopApp(code: string, state: string) {
  const callback = new URL("patchbay://auth/callback");
  callback.searchParams.set("code", code);
  callback.searchParams.set("state", state);
  window.location.href = callback.href;
}

/**
 * Validate that a CLI callback URL points to a safe host over HTTP.
 * Allows localhost and private/LAN IPs (RFC 1918) to support self-hosted setups
 * on local VMs while blocking arbitrary public hosts.
 */
export function validateCliCallback(cliCallback: string): boolean {
  try {
    const cbUrl = new URL(cliCallback);
    if (cbUrl.protocol !== "http:") return false;
    const h = cbUrl.hostname;
    if (h === "localhost" || h === "127.0.0.1") return true;
    // Allow RFC 1918 private IPs: 10.x.x.x, 172.16-31.x.x, 192.168.x.x
    if (/^10\./.test(h)) return true;
    if (/^172\.(1[6-9]|2\d|3[01])\./.test(h)) return true;
    if (/^192\.168\./.test(h)) return true;
    return false;
  } catch {
    return false;
  }
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function LoginPage({
  logo,
  onSuccess,
  cliCallback,
  onTokenObtained,
  onGoogleLogin,
  extra,
  embedded = false,
  showGoogleSeparator = false,
  googleLoading = false,
  externalError,
}: LoginPageProps) {
  const { t } = useT("auth");
  const qc = useQueryClient();
  const [step, setStep] = useState<"email" | "code" | "cli_confirm">("email");
  const [email, setEmail] = useState("");
  const [code, setCode] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [cooldown, setCooldown] = useState(0);
  const [existingUser, setExistingUser] = useState<User | null>(null);
  // Tracks how the existing session was detected so handleCliAuthorize
  // uses the matching token source (cookie → issueCliToken, localStorage → direct).
  const authSourceRef = useRef<"cookie" | "localStorage">("cookie");

  // Check for existing session when CLI callback is present.
  // Prioritises cookie auth (= current browser session) to avoid authorising
  // the CLI with a stale or mismatched localStorage token.
  useEffect(() => {
    if (!cliCallback) return;

    // Ensure no stale bearer token interferes — we want to test the cookie first.
    api.setToken(null);

    api
      .getMe()
      .then((user) => {
        authSourceRef.current = "cookie";
        setExistingUser(user);
        setStep("cli_confirm");
      })
      .catch(() => {
        // Cookie auth failed — fall back to localStorage token
        const token = localStorage.getItem("patchbay_token");
        if (!token) return;

        api.setToken(token);
        api
          .getMe()
          .then((user) => {
            authSourceRef.current = "localStorage";
            setExistingUser(user);
            setStep("cli_confirm");
          })
          .catch(() => {
            api.setToken(null);
            localStorage.removeItem("patchbay_token");
          });
      });
  }, [cliCallback]);

  // Cooldown timer for resend
  useEffect(() => {
    if (cooldown <= 0) return;
    const timer = setTimeout(() => setCooldown((c) => c - 1), 1000);
    return () => clearTimeout(timer);
  }, [cooldown]);

  const handleSendCode = useCallback(
    async (e?: React.FormEvent) => {
      e?.preventDefault();
      if (!email) {
        setError(t(($) => $.common.email_required));
        return;
      }
      setLoading(true);
      setError("");
      try {
        await useAuthStore.getState().sendCode(email);
        setStep("code");
        setCode("");
        setCooldown(60);
      } catch (err) {
        setError(
          err instanceof Error
            ? err.message
            : `${t(($) => $.errors.send_failed)} ${t(($) => $.errors.server_unreachable)}`,
        );
      } finally {
        setLoading(false);
      }
    },
    [email, t],
  );

  const handleVerify = useCallback(
    async (value: string) => {
      if (value.length !== 6) return;
      setLoading(true);
      setError("");
      try {
        if (cliCallback) {
          // CLI path: get token directly for the redirect URL
          const { token } = await api.verifyCode(email, value);
          localStorage.setItem("patchbay_token", token);
          api.setToken(token);
          onTokenObtained?.();
          redirectToCliCallback(cliCallback.url, token, cliCallback.state);
          return;
        }

        // Normal path: seed the workspace list into the Query cache so the
        // caller's onSuccess can read it synchronously to compute a destination
        // URL (first workspace's slug, or /workspaces/new for zero-workspace
        // users).
        await useAuthStore.getState().verifyCode(email, value);
        const wsList = await api.listWorkspaces();
        qc.setQueryData(workspaceKeys.list(), wsList);
        onTokenObtained?.();
        onSuccess();
      } catch (err) {
        setError(
          err instanceof Error
            ? err.message
            : t(($) => $.errors.code_invalid),
        );
        setCode("");
        setLoading(false);
      }
    },
    [email, onSuccess, cliCallback, onTokenObtained, qc, t],
  );

  const handleResend = async () => {
    if (cooldown > 0) return;
    setError("");
    try {
      await useAuthStore.getState().sendCode(email);
      setCooldown(60);
    } catch (err) {
      setError(
        err instanceof Error ? err.message : t(($) => $.errors.resend_failed),
      );
    }
  };

  const handleCliAuthorize = async () => {
    if (!cliCallback) return;
    setLoading(true);

    try {
      let token: string;

      if (authSourceRef.current === "localStorage") {
        // Session was detected via localStorage — reuse that token directly.
        const stored = localStorage.getItem("patchbay_token");
        if (!stored) throw new Error("token missing");
        token = stored;
      } else {
        // Session was detected via cookie — obtain a bearer token from the server.
        const res = await api.issueCliToken();
        token = res.token;
      }

      onTokenObtained?.();
      redirectToCliCallback(cliCallback.url, token, cliCallback.state);
    } catch {
      setError(t(($) => $.errors.cli_auth_failed));
      setExistingUser(null);
      setStep("email");
      setLoading(false);
    }
  };

  const handleGoogleLogin = () => {
    if (onGoogleLogin) {
      onGoogleLogin();
    }
  };

  const googleEnabled = Boolean(onGoogleLogin);
  const googleSeparator = showGoogleSeparator && googleEnabled ? (
    <FieldSeparator>{t(($) => $.common.or_continue_with)}</FieldSeparator>
  ) : null;
  const googleButton = googleEnabled ? (
    <Button
      type="button"
      variant="outline"
      onClick={handleGoogleLogin}
      disabled={loading || googleLoading}
      aria-busy={googleLoading}
    >
      <svg className="mr-2 h-4 w-4" viewBox="0 0 24 24" aria-hidden="true">
        <path
          d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92a5.06 5.06 0 0 1-2.2 3.32v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.1z"
          fill="#4285F4"
        />
        <path
          d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z"
          fill="#34A853"
        />
        <path
          d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z"
          fill="#FBBC05"
        />
        <path
          d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z"
          fill="#EA4335"
        />
      </svg>
      {googleLoading
        ? t(($) => $.desktop.entry.opening_google)
        : t(($) => $.signin.google)}
    </Button>
  ) : null;

  const stepHeading = (title: ReactNode, description: ReactNode) =>
    embedded ? (
      <div className="flex flex-col gap-2 text-center">
        <h1 className="text-2xl font-semibold tracking-tight">{title}</h1>
        <p className="text-sm text-muted-foreground">{description}</p>
      </div>
    ) : (
      <CardHeader className="text-center">
        {logo && <div className="mx-auto mb-4">{logo}</div>}
        <CardTitle className="text-display-sm">{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
    );

  // -------------------------------------------------------------------------
  // CLI confirm step
  // -------------------------------------------------------------------------

  if (step === "cli_confirm" && existingUser) {
    const cliBody = (
      <div className="flex flex-col gap-3">
        <Button
          onClick={handleCliAuthorize}
          disabled={loading}
          className="w-full"
          size="lg"
        >
          {loading
            ? t(($) => $.cli.authorizing)
            : t(($) => $.cli.authorize)}
        </Button>
        <Button
          variant="ghost"
          className="w-full"
          onClick={() => {
            setExistingUser(null);
            setStep("email");
          }}
        >
          {t(($) => $.cli.different_account)}
        </Button>
      </div>
    );

    if (embedded) {
      return (
        <div className="mx-auto flex w-full flex-col justify-center gap-6 sm:w-[350px]">
          {stepHeading(
            t(($) => $.cli.title),
            t(($) => $.cli.description, { email: existingUser.email }),
          )}
          {cliBody}
        </div>
      );
    }

    return (
      <div className="flex min-h-svh items-center justify-center">
        <Card className="w-full max-w-sm">
          {stepHeading(
            t(($) => $.cli.title),
            t(($) => $.cli.description, { email: existingUser.email }),
          )}
          <CardContent>{cliBody}</CardContent>
        </Card>
      </div>
    );
  }

  // -------------------------------------------------------------------------
  // Code verification step
  // -------------------------------------------------------------------------

  if (step === "code") {
    const codeBody = (
      <div className="flex flex-col items-center gap-4">
        <InputOTP
          autoFocus
          maxLength={6}
          value={code}
          onChange={(value) => {
            setCode(value);
            if (value.length === 6) handleVerify(value);
          }}
          disabled={loading}
          aria-label={t(($) => $.verify.title)}
        >
          <InputOTPGroup>
            <InputOTPSlot index={0} />
            <InputOTPSlot index={1} />
            <InputOTPSlot index={2} />
            <InputOTPSlot index={3} />
            <InputOTPSlot index={4} />
            <InputOTPSlot index={5} />
          </InputOTPGroup>
        </InputOTP>
        {error && <p className="text-body text-destructive">{error}</p>}
        <div className="flex items-center gap-2 text-body text-muted-foreground">
          <button
            type="button"
            onClick={handleResend}
            disabled={cooldown > 0}
            className="cursor-pointer text-primary underline-offset-4 hover:underline disabled:cursor-not-allowed disabled:text-muted-foreground disabled:no-underline"
          >
            {cooldown > 0
              ? t(($) => $.verify.resend_cooldown, { seconds: cooldown })
              : t(($) => $.verify.resend)}
          </button>
        </div>
      </div>
    );
    const codeBack = (
      <Button
        type="button"
        variant="ghost"
        className="w-full"
        onClick={() => {
          setStep("email");
          setCode("");
          setError("");
        }}
      >
        {t(($) => $.common.back)}
      </Button>
    );

    if (embedded) {
      return (
        <div className="mx-auto flex w-full flex-col justify-center gap-6 sm:w-[350px]">
          {stepHeading(
            t(($) => $.verify.title),
            t(($) => $.verify.description, { email }),
          )}
          {codeBody}
          {codeBack}
        </div>
      );
    }

    return (
      <div className="flex min-h-svh items-center justify-center">
        <Card className="w-full max-w-sm">
          {stepHeading(
            t(($) => $.verify.title),
            t(($) => $.verify.description, { email }),
          )}
          <CardContent>{codeBody}</CardContent>
          <CardFooter>{codeBack}</CardFooter>
        </Card>
      </div>
    );
  }

  // -------------------------------------------------------------------------
  // Email step
  // -------------------------------------------------------------------------

  const emailForm = embedded ? (
    <form id="login-form" onSubmit={handleSendCode}>
      <FieldGroup>
        <Field>
          <FieldLabel className="sr-only" htmlFor="login-email">
            {t(($) => $.common.email)}
          </FieldLabel>
          <Input
            id="login-email"
            type="email"
            placeholder={t(($) => $.common.email_placeholder)}
            autoCapitalize="none"
            autoComplete="email"
            autoCorrect="off"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            autoFocus
            required
            disabled={loading}
          />
          {error && <FieldError>{error}</FieldError>}
        </Field>
        <Field>
          <Button
            type="submit"
            disabled={!email || loading}
            aria-busy={loading}
          >
            {loading
              ? t(($) => $.signin.sending)
              : t(($) => $.signin.continue)}
          </Button>
        </Field>
      </FieldGroup>
    </form>
  ) : (
    <form id="login-form" onSubmit={handleSendCode} className="space-y-4">
      <div className="space-y-2">
        <Label htmlFor="login-email">{t(($) => $.common.email)}</Label>
        <Input
          id="login-email"
          type="email"
          placeholder={t(($) => $.common.email_placeholder)}
          value={email}
          onChange={(e) => setEmail(e.target.value)}
          autoFocus
          required
        />
      </div>
      {error && <p className="text-body text-destructive">{error}</p>}
    </form>
  );

  if (embedded) {
    return (
      <div className="mx-auto flex w-full flex-col justify-center gap-6 sm:w-[350px]">
        {externalError}
        {stepHeading(
          t(($) => $.desktop.entry.title),
          t(($) => $.desktop.entry.description),
        )}
        <div className="grid gap-6">
          {emailForm}
          {googleSeparator}
          {googleButton}
        </div>
        {extra && <div className="w-full pt-1 text-center">{extra}</div>}
      </div>
    );
  }

  return (
    <div className="flex min-h-svh items-center justify-center">
      <Card className="w-full max-w-sm">
        {stepHeading(
          t(($) => $.signin.title),
          t(($) => $.signin.description),
        )}
        <CardContent>{emailForm}</CardContent>
        <CardFooter className="flex flex-col gap-3">
          <Button
            type="submit"
            form="login-form"
            className="w-full"
            size="lg"
            disabled={!email || loading}
            aria-busy={loading}
          >
            {loading
              ? t(($) => $.signin.sending)
              : t(($) => $.signin.continue)}
          </Button>
          {googleSeparator}
          {googleButton}
          {extra && <div className="w-full pt-1 text-center">{extra}</div>}
        </CardFooter>
      </Card>
    </div>
  );
}
