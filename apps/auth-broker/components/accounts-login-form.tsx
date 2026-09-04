"use client";

import { useState } from "react";
import { useSignIn, useSignUp } from "@clerk/nextjs";
import { useAuthMessages } from "@/lib/auth-messages";
import { resolveAccountsReturnUrl } from "@/lib/redirect";

type AccountsLoginFormProps = {
  returnUrl: string;
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

export function buildGoogleLoginUrl(returnUrl: string, origin: string): string {
  const destination = new URL(
    resolveAccountsReturnUrl(returnUrl),
    origin,
  );
  const url = new URL("/oauth/google", origin);
  if (destination.searchParams.get("platform") === "desktop") {
    url.search = destination.search;
  } else {
    url.searchParams.set("return_url", destination.href);
  }
  return url.href;
}

export function AccountsLoginForm({ returnUrl }: AccountsLoginFormProps) {
  const messages = useAuthMessages();
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

  const reset = () => {
    setStep("email");
    setCode("");
    setError(null);
    void signIn?.reset();
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

  const handleGoogleLogin = () => {
    if (loading) return;
    window.location.assign(buildGoogleLoginUrl(returnUrl, window.location.origin));
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
        <h1>{messages.createAccount}</h1>
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
        <span className="accounts-login-form__provider-glyph" aria-hidden="true">
          G
        </span>
        {messages.google}
      </button>
      <p className="accounts-login-form__legal">{messages.terms}</p>
      <div id="clerk-captcha" />
    </div>
  );
}
