"use client";

import { useMemo } from "react";
import { CoreProvider } from "@patchbay/core/platform";
import { createBrowserCookieLocaleAdapter } from "@patchbay/core/i18n/browser";
import type { LocaleResources, SupportedLocale } from "@patchbay/core/i18n";
import { useWelcomeStore } from "@patchbay/core/onboarding";
import packageJson from "../package.json";
import { WebNavigationProvider } from "@/platform/navigation";
import { WebScrollRestorationProvider } from "@/platform/scroll-restoration";
import {
  setLoggedInCookie,
  clearLoggedInCookie,
} from "@/features/auth/auth-cookie";
import { detectWebOS } from "@/platform/client-os";
import { ClerkAuthAdapter } from "./clerk-auth-adapter";
import { UiFixturesProvider } from "@/lib/ui-fixtures/context";

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
  uiFixtures = false,
}: {
  children: React.ReactNode;
  locale: SupportedLocale;
  resources: Record<string, LocaleResources>;
  apiBaseUrl?: string;
  wsUrl?: string;
  uiFixtures?: boolean;
}) {
  // Clerk handles authentication on web unless local UI fixtures are serving
  // the product screens without a session.
  const clerkAuth = true;

  // Stable identity reference so downstream effects keyed on it don't see a
  // new object on every parent render.
  const identity = useMemo(
    () => ({ platform: "web", version: WEB_VERSION, os: detectWebOS() }),
    [],
  );
  const localeAdapter = useMemo(() => createBrowserCookieLocaleAdapter(), []);
  const tree = (
    <WebNavigationProvider>
      <WebScrollRestorationProvider>{children}</WebScrollRestorationProvider>
    </WebNavigationProvider>
  );
  return (
    <UiFixturesProvider enabled={uiFixtures}>
      <CoreProvider
        apiBaseUrl={apiBaseUrl}
        wsUrl={wsUrl || deriveWsUrl()}
        clerkAuth={clerkAuth}
        cookieAuth
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
        {uiFixtures ? tree : <ClerkAuthAdapter>{tree}</ClerkAuthAdapter>}
      </CoreProvider>
    </UiFixturesProvider>
  );
}
