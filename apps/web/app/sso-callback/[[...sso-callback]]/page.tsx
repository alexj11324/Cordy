"use client";

import { Suspense } from "react";
import { AuthenticateWithRedirectCallback } from "@clerk/nextjs";
import { buildDesktopHandoffQuery } from "@/features/auth/desktop-handoff";
import {
  authRouteWithRedirect,
  resolveSafeRedirectUrl,
} from "@/features/auth/safe-redirect";
import { useWebSearchParams } from "@/platform/client-navigation";

export default function SSOCallbackPage() {
  return (
    <Suspense>
      <SSOCallbackContent />
    </Suspense>
  );
}

function SSOCallbackContent() {
  const searchParams = useWebSearchParams();
  const desktopHandoff = searchParams.get("platform") === "desktop";
  const desktopHandoffQuery = desktopHandoff
    ? buildDesktopHandoffQuery(
        searchParams,
        process.env.NEXT_PUBLIC_DESKTOP_APP_ORIGIN,
      )
    : "";
  const redirectUrl = resolveSafeRedirectUrl(searchParams.get("redirect_url"));
  const loginUrl = desktopHandoff
    ? `/sign-in?${desktopHandoffQuery}`
    : authRouteWithRedirect("/sign-in", redirectUrl);
  const signUpUrl = desktopHandoff
    ? `/sign-up?${desktopHandoffQuery}`
    : authRouteWithRedirect("/sign-up", redirectUrl);
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
