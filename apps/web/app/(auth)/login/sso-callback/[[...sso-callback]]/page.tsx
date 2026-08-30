"use client";

import { Suspense } from "react";
import { AuthenticateWithRedirectCallback } from "@clerk/nextjs";
import { useSearchParams } from "next/navigation";
import { buildDesktopHandoffQuery } from "@/features/auth/desktop-handoff";
import {
  authRouteWithRedirect,
  resolveSafeRedirectUrl,
} from "@/features/auth/safe-redirect";

export default function SSOCallbackPage() {
  return (
    <Suspense>
      <SSOCallbackContent />
    </Suspense>
  );
}

function SSOCallbackContent() {
  const searchParams = useSearchParams();
  const desktopHandoff = searchParams.get("platform") === "desktop";
  const desktopHandoffQuery = desktopHandoff
    ? buildDesktopHandoffQuery(
        searchParams,
        process.env.NEXT_PUBLIC_DESKTOP_APP_ORIGIN,
      )
    : "";
  const redirectUrl = resolveSafeRedirectUrl(searchParams.get("redirect_url"));
  const loginUrl = desktopHandoff
    ? `/login?${desktopHandoffQuery}`
    : authRouteWithRedirect("/login", redirectUrl);
  const signUpUrl = desktopHandoff
    ? `/signup?${desktopHandoffQuery}`
    : authRouteWithRedirect("/signup", redirectUrl);
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
