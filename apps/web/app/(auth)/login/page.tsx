"use client";

import { SignIn } from "@clerk/nextjs";

export default function LoginPage() {
  return (
    <div className="flex min-h-screen items-center justify-center bg-background">
      <SignIn
        routing="path"
        path="/login"
        signUpUrl="/signup"
        forceRedirectUrl="/"
      />
    </div>
  );
}
