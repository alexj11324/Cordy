"use client";

import { Suspense } from "react";
import { SignUp } from "@clerk/nextjs";
import { ClerkAuthShell } from "@/components/clerk-auth-shell";
import {
  authRouteWithRedirect,
  resolveSafeRedirectUrl,
} from "@/features/auth/safe-redirect";
import { useWebSearchParams } from "@/platform/client-navigation";

export default function SignUpPage() {
  return (
    <Suspense>
      <SignUpContent />
    </Suspense>
  );
}

function SignUpContent() {
  const searchParams = useWebSearchParams();
  const redirectUrl = resolveSafeRedirectUrl(searchParams.get("redirect_url"));
  return (
    <ClerkAuthShell>
      <SignUp
        routing="path"
        path="/sign-up"
        signInUrl={authRouteWithRedirect("/sign-in", redirectUrl)}
        fallbackRedirectUrl={redirectUrl}
      />
    </ClerkAuthShell>
  );
}
