"use client";
import { createContext, useContext } from "react";
import { ClerkProvider } from "@clerk/nextjs";
import { AuthShell } from "./auth-shell";
import { useAuthMessages } from "@/lib/auth-messages";
import { AUTH_CONTRACT } from "@/lib/contract";
const ProductOriginContext = createContext<string>(AUTH_CONTRACT.origins.product);
export function useProductOrigin(): string {
  return useContext(ProductOriginContext);
}
export function RuntimeClerkProvider({
  children,
  publishableKey,
  productOrigin,
}: {
  children: React.ReactNode;
  publishableKey: string;
  productOrigin: string;
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
      <ProductOriginContext.Provider value={productOrigin}>
        {children}
      </ProductOriginContext.Provider>
    </ClerkProvider>
  );
}
