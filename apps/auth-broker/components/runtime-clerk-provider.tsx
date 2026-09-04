"use client";
import { ClerkProvider } from "@clerk/nextjs";
import { AuthShell } from "./auth-shell";
import { useAuthMessages } from "@/lib/auth-messages";
export function RuntimeClerkProvider({
  children,
  publishableKey,
}: {
  children: React.ReactNode;
  publishableKey: string;
}) {
  const messages = useAuthMessages();
  if (!publishableKey)
    return (
      <AuthShell>
        <p role="alert">{messages.unavailable}</p>
      </AuthShell>
    );
  return (
    <ClerkProvider publishableKey={publishableKey}>
      {children}
    </ClerkProvider>
  );
}
