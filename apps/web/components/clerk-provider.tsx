"use client";

import { ClerkProvider as BaseClerkProvider } from "@clerk/nextjs";

export function ClerkProvider({ children }: { children: React.ReactNode }) {
  return (
    <BaseClerkProvider
      publishableKey={process.env.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY}
    >
      {children}
    </BaseClerkProvider>
  );
}
