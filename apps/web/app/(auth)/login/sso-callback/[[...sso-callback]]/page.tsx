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
  const loginUrl = desktopHandoff ? "/login?platform=desktop" : "/login";
  const signUpUrl = desktopHandoff ? "/signup?platform=desktop" : "/signup";
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
