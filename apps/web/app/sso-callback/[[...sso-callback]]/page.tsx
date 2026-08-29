"use client";

import { Suspense } from "react";
import { AuthenticateWithRedirectCallback } from "@clerk/nextjs";
import { useSearchParams } from "next/navigation";

export default function SSOCallbackPage() {
  return (
    <Suspense>
      <SSOCallbackContent />
    </Suspense>
  );
}

function SSOCallbackContent() {
  const desktopHandoff = useSearchParams().get("platform") === "desktop";
  const loginUrl = desktopHandoff ? "/sign-in?platform=desktop" : "/sign-in";
  const signUpUrl = desktopHandoff ? "/sign-up?platform=desktop" : "/sign-up";
  const returnUrl = desktopHandoff ? "/login?platform=desktop" : "/";

  return (
    <AuthenticateWithRedirectCallback
      signInUrl={loginUrl}
      signUpUrl={signUpUrl}
      signInFallbackRedirectUrl={returnUrl}
      signUpFallbackRedirectUrl={returnUrl}
    />
  );
}
