"use client";

import { Suspense } from "react";
import { SignIn } from "@clerk/nextjs";
import { useSearchParams } from "next/navigation";
import { ClerkAuthShell } from "@/components/clerk-auth-shell";

export default function SignInPage() {
  return (
    <Suspense>
      <SignInContent />
    </Suspense>
  );
}

function SignInContent() {
  const desktopHandoff = useSearchParams().get("platform") === "desktop";
  return (
    <ClerkAuthShell>
      <SignIn
        routing="path"
        path="/sign-in"
        signUpUrl={desktopHandoff ? "/sign-up?platform=desktop" : "/sign-up"}
        fallbackRedirectUrl={desktopHandoff ? "/login?platform=desktop" : "/"}
      />
    </ClerkAuthShell>
  );
}
