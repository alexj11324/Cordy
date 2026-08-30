"use client";

import { Suspense } from "react";
import { SignUp } from "@clerk/nextjs";
import { useSearchParams } from "next/navigation";
import { ClerkAuthShell } from "@/components/clerk-auth-shell";
import { buildDesktopHandoffQuery } from "@/features/auth/desktop-handoff";
import {
  authRouteWithRedirect,
  resolveSafeRedirectUrl,
} from "@/features/auth/safe-redirect";

export default function SignUpPage() {
  return (
    <Suspense>
      <SignUpContent />
    </Suspense>
  );
}

function SignUpContent() {
  const searchParams = useSearchParams();
  const desktopHandoff = searchParams.get("platform") === "desktop";
  const desktopHandoffQuery = desktopHandoff
    ? buildDesktopHandoffQuery(searchParams)
    : "";
  const redirectUrl = resolveSafeRedirectUrl(searchParams.get("redirect_url"));
  return (
    <ClerkAuthShell>
      <SignUp
        routing="path"
        path="/signup"
        signInUrl={
          desktopHandoff
            ? `/login?${desktopHandoffQuery}`
            : authRouteWithRedirect("/login", redirectUrl)
        }
        fallbackRedirectUrl={
          desktopHandoff ? `/login?${desktopHandoffQuery}` : redirectUrl
        }
      />
    </ClerkAuthShell>
  );
}
