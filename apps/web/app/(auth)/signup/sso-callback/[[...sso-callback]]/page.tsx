"use client";

import { Suspense } from "react";
import { ClerkSSOCallback } from "@/components/clerk-sso-callback";

export default function LegacySignUpSSOCallbackPage() {
  return (
    <Suspense>
      <ClerkSSOCallback signInPath="/login" signUpPath="/signup" />
    </Suspense>
  );
}
