import type { Metadata } from "next";
import { headers } from "next/headers";
import { RuntimeClerkProvider } from "@/components/runtime-clerk-provider";
import { resolveAuthLocale } from "@/lib/auth-locale";
import { readAuthBrokerRuntimeConfig } from "@/lib/runtime-config";
import "./globals.css";
export const dynamic = "force-dynamic";
export const metadata: Metadata = { title: "Sign in · Patchbay", robots: { index: false, follow: false } };
export default async function RootLayout({ children }: { children: React.ReactNode }) {
  const runtime = readAuthBrokerRuntimeConfig();
  const locale = resolveAuthLocale((await headers()).get("accept-language"));
  return <html lang={locale.htmlLang}><body><RuntimeClerkProvider publishableKey={runtime.ok ? runtime.config.clerkPublishableKey : ""}>{children}</RuntimeClerkProvider></body></html>;
}
