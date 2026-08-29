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
        path="/signup"
        signInUrl={desktopHandoff ? "/login?platform=desktop" : "/login"}
        fallbackRedirectUrl={desktopHandoff ? "/login?platform=desktop" : "/"}
      />
    </ClerkAuthShell>
  );
}
