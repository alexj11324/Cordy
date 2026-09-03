"use client";

import { ClerkProvider as BaseClerkProvider } from "@clerk/nextjs";
import { shadcn } from "@clerk/themes";

export function ClerkProvider({ children }: { children: React.ReactNode }) {
  return (
    <BaseClerkProvider
      publishableKey={process.env.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY}
      appearance={{ theme: shadcn }}
    >
      {children}
    </BaseClerkProvider>
  );
}
