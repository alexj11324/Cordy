"use client";

import { Suspense } from "react";
import { AuthenticateWithRedirectCallback } from "@clerk/nextjs";
import { useSearchParams } from "next/navigation";
import { buildDesktopHandoffQuery } from "@/features/auth/desktop-handoff";

export default function LegacySignUpSSOCallbackPage() {
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
    ? buildDesktopHandoffQuery(searchParams)
    : "";
  const loginUrl = desktopHandoff
    ? `/login?${desktopHandoffQuery}`
    : "/login";
  const signUpUrl = desktopHandoff
    ? `/signup?${desktopHandoffQuery}`
    : "/signup";
  const returnUrl = desktopHandoff
    ? `/login?${desktopHandoffQuery}`
    : "/";

  return (
    <AuthenticateWithRedirectCallback
      signInUrl={loginUrl}
      signUpUrl={signUpUrl}
      signInFallbackRedirectUrl={returnUrl}
      signUpFallbackRedirectUrl={returnUrl}
    />
  );
}
