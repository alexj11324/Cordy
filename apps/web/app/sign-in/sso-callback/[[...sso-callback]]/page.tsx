"use client";

import { Suspense } from "react";
import { ClerkSSOCallback } from "@/components/clerk-sso-callback";

export default function SignInSSOCallbackPage() {
  return (
    <Suspense>
      <ClerkSSOCallback signInPath="/sign-in" signUpPath="/sign-up" />
    </Suspense>
  );
}
