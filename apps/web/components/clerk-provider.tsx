"use client";

import { ClerkProvider as BaseClerkProvider } from "@clerk/nextjs";
import { shadcn } from "@clerk/themes";

export function ClerkProvider({
  children,
  publishableKey,
}: {
  children: React.ReactNode;
  publishableKey?: string;
}) {
  return (
    <BaseClerkProvider
      publishableKey={publishableKey}
      appearance={{ theme: shadcn }}
    >
      {children}
    </BaseClerkProvider>
  );
}
