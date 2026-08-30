"use client";

import { Suspense } from "react";
import { AuthenticateWithRedirectCallback } from "@clerk/nextjs";
import { useSearchParams } from "next/navigation";
import { buildDesktopHandoffQuery } from "@/features/auth/desktop-handoff";

export default function SignUpSSOCallbackPage() {
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
    ? `/sign-in?${desktopHandoffQuery}`
    : "/sign-in";
  const signUpUrl = desktopHandoff
    ? `/sign-up?${desktopHandoffQuery}`
    : "/sign-up";
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
