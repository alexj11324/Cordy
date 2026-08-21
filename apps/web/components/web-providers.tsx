"use client";

import { useMemo } from "react";
import { CoreProvider } from "@cordy/core/platform";
import { createBrowserCookieLocaleAdapter } from "@cordy/core/i18n/browser";
import type { LocaleResources, SupportedLocale } from "@cordy/core/i18n";
import { useWelcomeStore } from "@cordy/core/onboarding";
import packageJson from "../package.json";
import { WebNavigationProvider } from "@/platform/navigation";
import { WebScrollRestorationProvider } from "@/platform/scroll-restoration";
import {
  setLoggedInCookie,
  clearLoggedInCookie,
} from "@/features/auth/auth-cookie";
import { detectWebOS } from "@/platform/client-os";
import { ClerkAuthAdapter } from "./clerk-auth-adapter";

// Derive WebSocket URL from the page origin so self-hosted / LAN deployments
// work without an explicit runtime wsUrl. The Next.js runtime proxy handles
// /ws -> backend when the deployment keeps WebSockets same-origin.
function deriveWsUrl(): string | undefined {
  if (typeof window === "undefined") return undefined;
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${window.location.host}/ws`;
}

// Build-time version preferred (CI sets NEXT_PUBLIC_APP_VERSION to a git tag
// or sha so different deploys are distinguishable in server logs); fall back
// to the package.json version so local dev still reports something useful.
const WEB_VERSION =
  process.env.NEXT_PUBLIC_APP_VERSION || packageJson.version || "dev";

export function WebProviders({
  children,
  locale,
  resources,
  apiBaseUrl,
  wsUrl,
}: {
  children: React.ReactNode;
  locale: SupportedLocale;
  resources: Record<string, LocaleResources>;
  apiBaseUrl?: string;
  wsUrl?: string;
}) {
  // Clerk handles all authentication on web — skip legacy cookie/token logic.
  const clerkAuth = true;

  // Stable identity reference so downstream effects keyed on it don't see a
  // new object on every parent render.
  const identity = useMemo(
    () => ({ platform: "web", version: WEB_VERSION, os: detectWebOS() }),
    [],
  );
  const localeAdapter = useMemo(() => createBrowserCookieLocaleAdapter(), []);
  return (
    <CoreProvider
      apiBaseUrl={apiBaseUrl}
      wsUrl={wsUrl || deriveWsUrl()}
      clerkAuth={clerkAuth}
      cookieAuth={false}
      onLogin={setLoggedInCookie}
      onLogout={() => {
        useWelcomeStore.getState().reset();
        clearLoggedInCookie();
      }}
      identity={identity}
      locale={locale}
      resources={resources}
      localeAdapter={localeAdapter}
    >
      <ClerkAuthAdapter>
        <WebNavigationProvider>
          <WebScrollRestorationProvider>{children}</WebScrollRestorationProvider>
        </WebNavigationProvider>
      </ClerkAuthAdapter>
    </CoreProvider>
  );
}
