"use client";

import { AuthenticateWithRedirectCallback } from "@clerk/nextjs";
import { buildDesktopHandoffQuery } from "@/features/auth/desktop-handoff";
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
  const desktopHandoff = searchParams.get("platform") === "desktop";

  const desktopHandoffQuery = desktopHandoff
    ? buildDesktopHandoffQuery(searchParams)
    : "";
  const redirectUrl = resolveSafeRedirectUrl(searchParams.get("redirect_url"));
  const loginUrl = desktopHandoff
    ? `${signInPath}?${desktopHandoffQuery}`
    : authRouteWithRedirect(signInPath, redirectUrl);
  const signUpUrl = desktopHandoff
    ? `${signUpPath}?${desktopHandoffQuery}`
    : authRouteWithRedirect(signUpPath, redirectUrl);
  const returnUrl = desktopHandoff
    ? `/login?${desktopHandoffQuery}`
    : redirectUrl;

  return (
    <AuthenticateWithRedirectCallback
      signInUrl={loginUrl}
      signUpUrl={signUpUrl}
      signInFallbackRedirectUrl={returnUrl}
      signUpFallbackRedirectUrl={returnUrl}
    />
  );
}
