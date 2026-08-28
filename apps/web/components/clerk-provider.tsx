"use client";

import { ClerkProvider as BaseClerkProvider } from "@clerk/nextjs";

export function ClerkProvider({
  children,
  publishableKey,
}: {
  children: React.ReactNode;
  publishableKey?: string;
}) {
  return (
    <BaseClerkProvider {...(publishableKey ? { publishableKey } : {})}>
      {children}
    </BaseClerkProvider>
  );
}
