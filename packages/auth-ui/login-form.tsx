"use client";

import { useEffect, useState } from "react";
import { useSignIn, useSignUp } from "@clerk/nextjs";
import type { AuthMessages } from "./messages";

type AccountsLoginFormProps = {
  messages: AuthMessages;
  onGoogleLogin: () => void | Promise<void>;
};

type ClerkErrorItem = {
  code?: string;
  longMessage?: string;
  message?: string;
};

type ClerkErrorShape = {
  errors?: ClerkErrorItem[];
  longMessage?: string;
  message?: string;
};

function asClerkError(value: unknown): ClerkErrorShape | null {
  if (!value || typeof value !== "object") return null;
  const candidate = value as Record<string, unknown>;
  const errors = Array.isArray(candidate.errors)
    ? candidate.errors.filter(
        (item): item is ClerkErrorItem =>
          Boolean(item) && typeof item === "object",
      )
    : undefined;
  return {
    errors,
    longMessage:
      typeof candidate.longMessage === "string"
        ? candidate.longMessage
        : undefined,
    message:
      typeof candidate.message === "string" ? candidate.message : undefined,
  };
}

function clerkErrorMessage(value: unknown, fallback: string): string {
  const error = asClerkError(value);
  const message =
    error?.errors?.find((item) => item.longMessage || item.message)?.longMessage ??
    error?.errors?.find((item) => item.longMessage || item.message)?.message ??
    error?.longMessage ??
    error?.message ??
    (value instanceof Error ? value.message : "");
  return message || fallback;
}

function clerkErrorCode(value: unknown): string | null {
  return asClerkError(value)?.errors?.find((item) => item.code)?.code ?? null;
}

function GoogleMark() {
  return (
    <svg
      data-testid="google-mark"
      className="accounts-login-form__provider-glyph"
      viewBox="0 0 24 24"
      aria-hidden="true"
    >
      <path
        fill="#4285F4"
        d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92c-.26 1.37-1.04 2.53-2.21 3.31v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.09z"
      />
      <path
        fill="#34A853"
        d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z"
      />
      <path
        fill="#FBBC05"
        d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z"
      />
      <path
        fill="#EA4335"
        d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z"
      />
    </svg>
  );
}

export function AccountsLoginForm({ messages, onGoogleLogin }: AccountsLoginFormProps) {
  const { signIn } = useSignIn();
  const { signUp } = useSignUp();
  const [email, setEmail] = useState("");
  const [code, setCode] = useState("");
  const [step, setStep] = useState<"email" | "code" | "requirements">(
    "email",
  );
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [legalAccepted, setLegalAccepted] = useState(false);

  useEffect(() => {
    if (signUp?.status === "missing_requirements" && signUp.verifications.externalAccount.status === "verified") {
      setStep("requirements");
    }
  }, [signUp?.status, signUp?.verifications.externalAccount.status]);

  const reset = () => {
    setStep("email");
    setCode("");
    setError(null);
    void signIn?.reset();
    void signUp?.reset();
  };

  const finishSignIn = async () => {
    if (!signIn) throw new Error("Clerk sign-in is unavailable");
    const result = await signIn.finalize({
      navigate: async ({ session }) => {
        if (session.currentTask) {
          throw new Error(messages.authTaskRequired);
        }
      },
    });
    if (result.error) throw result.error;
  };

  const finishSignUp = async () => {
    if (!signUp) throw new Error("Clerk sign-up is unavailable");
    const result = await signUp.finalize({
      navigate: async ({ session }) => {
        if (session.currentTask) {
          throw new Error(messages.authTaskRequired);
        }
      },
    });
    if (result.error) throw result.error;
  };

  const transferToSignUp = async () => {
    if (!signUp) throw new Error("Clerk sign-up is unavailable");
    const result = await signUp.create({ transfer: true });
    if (result.error) throw result.error;
    if (signUp.status === "complete") {
      await finishSignUp();
      return;
    }
    if (signUp.status === "missing_requirements") {
      setStep("requirements");
      return;
    }
    throw new Error(messages.missingRequirements);
  };

  const handleEmailSubmit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!signIn || !signUp) return;
    const normalizedEmail = email.trim();
    if (!normalizedEmail) {
      setError(messages.emailRequired);
      return;
    }

    setLoading(true);
    setError(null);
    try {
      const result = await signIn.create({
        identifier: normalizedEmail,
        signUpIfMissing: true,
      });
      if (result.error) throw result.error;
      const sendCode = await signIn.emailCode.sendCode();
      if (sendCode.error) throw sendCode.error;
      setEmail(normalizedEmail);
      setStep("code");
    } catch (failure) {
      setError(clerkErrorMessage(failure, messages.authError));
    } finally {
      setLoading(false);
    }
  };

  const handleVerificationSubmit = async (
    event: React.FormEvent<HTMLFormElement>,
  ) => {
    event.preventDefault();
    if (!signIn || !signUp || !code.trim()) return;

    setLoading(true);
    setError(null);
    try {
      const result = await signIn.emailCode.verifyCode({ code: code.trim() });
      if (result.error) {
        if (clerkErrorCode(result.error) === "sign_up_if_missing_transfer") {
          await transferToSignUp();
          return;
        }
        throw result.error;
      }
      if (signIn.status === "complete") {
        await finishSignIn();
        return;
      }
      throw new Error(messages.verificationIncomplete);
    } catch (failure) {
      setError(clerkErrorMessage(failure, messages.verificationError));
    } finally {
      setLoading(false);
    }
  };

  const handleRequirementsSubmit = async (
    event: React.FormEvent<HTMLFormElement>,
  ) => {
    event.preventDefault();
    if (!signUp || !legalAccepted) return;

    setLoading(true);
    setError(null);
    try {
      const result = await signUp.update({ legalAccepted: true });
      if (result.error) throw result.error;
      if (signUp.status === "complete") {
        await finishSignUp();
        return;
      }
      throw new Error(messages.missingRequirements);
    } catch (failure) {
      setError(clerkErrorMessage(failure, messages.authError));
    } finally {
      setLoading(false);
    }
  };

  const handleGoogleLogin = async () => {
    if (loading) return;
    setLoading(true);
    setError(null);
    try { await onGoogleLogin(); }
    catch (failure) { setError(clerkErrorMessage(failure, messages.startFailed)); }
    finally { setLoading(false); }
  };

  if (step === "code") {
    return (
      <div className="accounts-login-form" data-testid="accounts-login-form">
        <div className="accounts-login-form__heading">
          <h1>{messages.verifyTitle}</h1>
          <p>{messages.verifyDescription.replace("{{email}}", email)}</p>
        </div>
        <form
          className="accounts-login-form__fields"
          onSubmit={handleVerificationSubmit}
        >
          <div className="accounts-login-form__field">
            <label htmlFor="accounts-verification-code">
              {messages.verificationCode}
            </label>
            <input
              id="accounts-verification-code"
              inputMode="numeric"
              autoComplete="one-time-code"
              maxLength={8}
              value={code}
              onChange={(event) => setCode(event.target.value)}
              disabled={loading}
              required
            />
          </div>
          {error && (
            <p className="accounts-login-form__error" role="alert">
              {error}
            </p>
          )}
          <button
            type="submit"
            className="accounts-login-form__button accounts-login-form__button--primary"
            disabled={loading || !code.trim()}
            aria-busy={loading}
          >
            {messages.verifyButton}
          </button>
        </form>
        <div className="accounts-login-form__footer">
          <button
            type="button"
            className="accounts-login-form__link"
            onClick={() => {
              setLoading(true);
              setError(null);
              void signIn?.emailCode
                .sendCode()
                .then((result) => {
                  if (result.error) {
                    setError(clerkErrorMessage(result.error, messages.authError));
                  }
                })
                .finally(() => setLoading(false));
            }}
            disabled={loading}
          >
            {messages.resend}
          </button>
          <button
            type="button"
            className="accounts-login-form__link"
            onClick={reset}
            disabled={loading}
          >
            {messages.back}
          </button>
        </div>
        <div id="clerk-captcha" />
      </div>
    );
  }

  if (step === "requirements") {
    return (
      <div className="accounts-login-form" data-testid="accounts-login-form">
        <div className="accounts-login-form__heading">
          <h1>{messages.completeAccount}</h1>
          <p>{messages.completeAccountDescription}</p>
        </div>
        <form
          className="accounts-login-form__requirements"
          onSubmit={handleRequirementsSubmit}
        >
          <label className="accounts-login-form__legal-check">
            <input
              type="checkbox"
              checked={legalAccepted}
              onChange={(event) => setLegalAccepted(event.target.checked)}
              disabled={loading}
              required
            />
            <span>{messages.legal}</span>
          </label>
          {error && (
            <p className="accounts-login-form__error" role="alert">
              {error}
            </p>
          )}
          <button
            type="submit"
            className="accounts-login-form__button accounts-login-form__button--primary"
            disabled={loading || !legalAccepted}
            aria-busy={loading}
          >
            {messages.createAccountButton}
          </button>
        </form>
        <button
          type="button"
          className="accounts-login-form__link"
          onClick={reset}
          disabled={loading}
        >
          {messages.startOver}
        </button>
      </div>
    );
  }

  return (
    <div className="accounts-login-form" data-testid="accounts-login-form">
      <div className="accounts-login-form__heading">
        <h1>{messages.login}</h1>
        <p>{messages.emailDescription}</p>
      </div>
      <form className="accounts-login-form__fields" onSubmit={handleEmailSubmit}>
        <div className="accounts-login-form__field">
          <label className="sr-only" htmlFor="accounts-email">
            {messages.email}
          </label>
          <input
            id="accounts-email"
            type="email"
            autoComplete="email"
            autoCapitalize="none"
            autoCorrect="off"
            placeholder={messages.emailPlaceholder}
            value={email}
            onChange={(event) => setEmail(event.target.value)}
            disabled={loading || !signIn}
            required
          />
        </div>
        {error && (
          <p className="accounts-login-form__error" role="alert">
            {error}
          </p>
        )}
        <button
          type="submit"
          className="accounts-login-form__button accounts-login-form__button--primary"
          disabled={loading || !signIn || !email.trim()}
          aria-busy={loading}
        >
          {messages.emailButton}
        </button>
      </form>
      <div className="accounts-login-form__separator" role="separator">
        <span>{messages.continueWith}</span>
      </div>
      <button
        type="button"
        className="accounts-login-form__button accounts-login-form__button--secondary"
        onClick={handleGoogleLogin}
        disabled={loading || !signIn}
        aria-busy={loading}
      >
        <GoogleMark />
        {messages.google}
      </button>
      <p className="accounts-login-form__legal">{messages.terms}</p>
      <div id="clerk-captcha" />
    </div>
  );
}
