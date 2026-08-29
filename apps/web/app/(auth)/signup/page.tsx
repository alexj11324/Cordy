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
  const guestTransfer = searchParams.get("guest_transfer") ?? "";
  const desktopAuthQuery = new URLSearchParams({
    ...(desktopHandoff ? { platform: "desktop" } : {}),
    ...(guestTransfer ? { guest_transfer: guestTransfer } : {}),
  }).toString();
  const authQuery = desktopAuthQuery ? `?${desktopAuthQuery}` : "";
  return (
    <ClerkAuthShell>
      <SignUp
        routing="path"
        path="/signup"
        signInUrl={`/login${authQuery}`}
        fallbackRedirectUrl={desktopHandoff ? `/login${authQuery}` : "/"}
      />
    </ClerkAuthShell>
  );
}
