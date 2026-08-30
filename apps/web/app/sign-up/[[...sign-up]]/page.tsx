"use client";

import { Suspense } from "react";
import { SignUp } from "@clerk/nextjs";
import { useSearchParams } from "next/navigation";
import { ClerkAuthShell } from "@/components/clerk-auth-shell";
import { buildDesktopHandoffQuery } from "@/features/auth/desktop-handoff";

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
  return (
    <ClerkAuthShell>
      <SignUp
        routing="path"
        path="/sign-up"
        signInUrl={
          desktopHandoff ? `/sign-in?${desktopHandoffQuery}` : "/sign-in"
        }
        fallbackRedirectUrl={
          desktopHandoff ? `/login?${desktopHandoffQuery}` : "/"
        }
      />
    </ClerkAuthShell>
  );
}
