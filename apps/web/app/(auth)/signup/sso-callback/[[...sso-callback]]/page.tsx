"use client";

import { AuthenticateWithRedirectCallback } from "@clerk/nextjs";

export default function LegacySignUpSSOCallbackPage() {
  return (
    <AuthenticateWithRedirectCallback
      signInUrl="/login"
      signUpUrl="/signup"
      signInFallbackRedirectUrl="/"
      signUpFallbackRedirectUrl="/"
    />
  );
}
