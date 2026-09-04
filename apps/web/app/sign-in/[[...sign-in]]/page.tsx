"use client";

import { Suspense } from "react";
import { SignIn } from "@clerk/nextjs";
import { ClerkAuthShell } from "@/components/clerk-auth-shell";
import {
  authRouteWithRedirect,
  resolveSafeRedirectUrl,
} from "@/features/auth/safe-redirect";
import { useWebSearchParams } from "@/platform/client-navigation";

export default function SignInPage() {
  return (
    <Suspense>
      <SignInContent />
    </Suspense>
  );
}

function SignInContent() {
  const searchParams = useWebSearchParams();
  const redirectUrl = resolveSafeRedirectUrl(searchParams.get("redirect_url"));
  return (
    <ClerkAuthShell>
      <SignIn
        routing="path"
        path="/sign-in"
        signUpUrl={authRouteWithRedirect("/sign-up", redirectUrl)}
        fallbackRedirectUrl={redirectUrl}
      />
    </ClerkAuthShell>
  );
}
