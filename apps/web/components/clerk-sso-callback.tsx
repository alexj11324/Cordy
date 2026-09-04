"use client";

import { AuthenticateWithRedirectCallback } from "@clerk/nextjs";
import {
  authRouteWithRedirect,
  resolveSafeRedirectUrl,
} from "@/features/auth/safe-redirect";
import { useWebSearchParams } from "@/platform/client-navigation";

type ClerkSSOCallbackProps = {
  signInPath: "/login" | "/sign-in";
  signUpPath: "/signup" | "/sign-up";
};

export function ClerkSSOCallback({
  signInPath,
  signUpPath,
}: ClerkSSOCallbackProps) {
  const searchParams = useWebSearchParams();
  const redirectUrl = resolveSafeRedirectUrl(searchParams.get("redirect_url"));
  const loginUrl = authRouteWithRedirect(signInPath, redirectUrl);
  const signUpUrl = authRouteWithRedirect(signUpPath, redirectUrl);
  const returnUrl = redirectUrl;

  return (
    <AuthenticateWithRedirectCallback
      signInUrl={loginUrl}
      signUpUrl={signUpUrl}
      signInFallbackRedirectUrl={returnUrl}
      signUpFallbackRedirectUrl={returnUrl}
    />
  );
}
