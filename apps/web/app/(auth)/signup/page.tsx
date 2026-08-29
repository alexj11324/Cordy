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
  const searchParams = useSearchParams();
  const desktopHandoff = searchParams.get("platform") === "desktop";
  const desktopHandoffQuery = desktopHandoff
    ? buildDesktopHandoffQuery(searchParams)
    : "";
  return (
    <ClerkAuthShell>
      <SignUp
        routing="path"
        path="/signup"
        signInUrl={
          desktopHandoff ? `/login?${desktopHandoffQuery}` : "/login"
        }
        fallbackRedirectUrl={
          desktopHandoff ? `/login?${desktopHandoffQuery}` : "/"
        }
      />
    </ClerkAuthShell>
  );
}

function buildDesktopHandoffQuery(searchParams: URLSearchParams): string {
  const params = new URLSearchParams({ platform: "desktop" });
  for (const key of ["code_challenge", "state"] as const) {
    const value = searchParams.get(key);
    if (value) params.set(key, value);
  }
  return params.toString();
}
