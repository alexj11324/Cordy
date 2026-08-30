"use client";

import { Suspense } from "react";
import { SignUp } from "@clerk/nextjs";
import { ClerkAuthShell } from "@/components/clerk-auth-shell";
import { buildDesktopHandoffQuery } from "@/features/auth/desktop-handoff";
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
  const desktopHandoff = searchParams.get("platform") === "desktop";
  const desktopHandoffQuery = desktopHandoff
    ? buildDesktopHandoffQuery(searchParams)
    : "";
  const redirectUrl = resolveSafeRedirectUrl(searchParams.get("redirect_url"));
  return (
    <ClerkAuthShell>
      <SignUp
        routing="path"
        path="/sign-up"
        signInUrl={
          desktopHandoff
            ? `/sign-in?${desktopHandoffQuery}`
            : authRouteWithRedirect("/sign-in", redirectUrl)
        }
        fallbackRedirectUrl={
          desktopHandoff ? `/login?${desktopHandoffQuery}` : redirectUrl
        }
      />
    </ClerkAuthShell>
  );
}
