import type { Metadata } from "next";
import { RuntimeClerkProvider } from "@/components/runtime-clerk-provider";
import { readAuthBrokerRuntimeConfig } from "@/lib/runtime-config";
import "./globals.css";
export const dynamic = "force-dynamic";
export const metadata: Metadata = { title: "Sign in · Patchbay", robots: { index: false, follow: false } };
export default function RootLayout({ children }: { children: React.ReactNode }) { const runtime = readAuthBrokerRuntimeConfig(); return <html lang="en"><body><RuntimeClerkProvider publishableKey={runtime.ok ? runtime.config.clerkPublishableKey : ""}>{children}</RuntimeClerkProvider></body></html>; }
