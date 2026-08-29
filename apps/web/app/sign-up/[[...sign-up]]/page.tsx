"use client";

import { Suspense } from "react";
import { SignUp } from "@clerk/nextjs";
import { useSearchParams } from "next/navigation";
import { ClerkAuthShell } from "@/components/clerk-auth-shell";

export default function SignUpPage() {
  return (
    <Suspense>
      <SignUpContent />
    </Suspense>
  );
}

function SignUpContent() {
  const desktopHandoff = useSearchParams().get("platform") === "desktop";
  return (
    <ClerkAuthShell>
      <SignUp
        routing="path"
        path="/sign-up"
        signInUrl={desktopHandoff ? "/sign-in?platform=desktop" : "/sign-in"}
        fallbackRedirectUrl={desktopHandoff ? "/login?platform=desktop" : "/"}
      />
    </ClerkAuthShell>
  );
}
