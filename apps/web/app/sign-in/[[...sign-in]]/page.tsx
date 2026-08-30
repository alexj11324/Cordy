"use client";

import { Suspense } from "react";
import { SignIn } from "@clerk/nextjs";
import { useSearchParams } from "next/navigation";
import { ClerkAuthShell } from "@/components/clerk-auth-shell";
import { buildDesktopHandoffQuery } from "@/features/auth/desktop-handoff";

export default function SignInPage() {
  return (
    <Suspense>
      <SignInContent />
    </Suspense>
  );
}

function SignInContent() {
  const searchParams = useSearchParams();
  const desktopHandoff = searchParams.get("platform") === "desktop";
  const desktopHandoffQuery = desktopHandoff
    ? buildDesktopHandoffQuery(searchParams)
    : "";
  return (
    <ClerkAuthShell>
      <SignIn
        routing="path"
        path="/sign-in"
        signUpUrl={
          desktopHandoff ? `/sign-up?${desktopHandoffQuery}` : "/sign-up"
        }
        fallbackRedirectUrl={
          desktopHandoff ? `/login?${desktopHandoffQuery}` : "/"
        }
      />
    </ClerkAuthShell>
  );
}
