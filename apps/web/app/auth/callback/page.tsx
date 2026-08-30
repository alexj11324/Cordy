"use client";

import { Suspense, useEffect } from "react";
import { ClerkAuthShell } from "@/components/clerk-auth-shell";
import { useT } from "@patchbay/views/i18n";
import {
  buildDesktopHandoffQuery,
  hasInvalidDesktopBrowserAppOrigin,
} from "@/features/auth/desktop-handoff";
import {
  useWebRouter,
  useWebSearchParams,
} from "@/platform/client-navigation";

function resolveSafeRedirectUrl(raw: string | null): string {
  if (!raw) return "/";
  if (raw.startsWith("/") && !raw.startsWith("//")) {
    const url = new URL(raw, "https://patchbay.invalid");
    return `${url.pathname}${url.search}${url.hash}` || "/";
  }
  try {
    const url = new URL(raw);
    if (url.origin !== window.location.origin) return "/";
    return `${url.pathname}${url.search}${url.hash}` || "/";
  } catch {
    return "/";
  }
}

/**
 * Compatibility callback for existing provider registrations. Clerk's current
 * OAuth flow uses the dedicated SSO callback route below `/login` or `/sign-in`.
 */
export default function AuthCallbackPage() {
  return (
    <Suspense>
      <AuthCallbackContent />
    </Suspense>
  );
}

function AuthCallbackContent() {
  const router = useWebRouter();
  const searchParams = useWebSearchParams();
  const { t } = useT("auth");
  const desktopHandoff = searchParams.get("platform") === "desktop";
  const invalidDesktopAppOrigin =
    desktopHandoff &&
    hasInvalidDesktopBrowserAppOrigin(
      searchParams,
      process.env.NEXT_PUBLIC_DESKTOP_APP_ORIGIN,
    );

  useEffect(() => {
    if (invalidDesktopAppOrigin) return;
    if (desktopHandoff) {
      router.replace(
        `/login?${buildDesktopHandoffQuery(
          searchParams,
          process.env.NEXT_PUBLIC_DESKTOP_APP_ORIGIN,
        )}`,
      );
      return;
    }
    const redirectUrl = searchParams.get("redirect_url");
    router.replace(resolveSafeRedirectUrl(redirectUrl));
  }, [desktopHandoff, invalidDesktopAppOrigin, router, searchParams]);

  if (invalidDesktopAppOrigin) {
    return (
      <ClerkAuthShell>
        <p role="alert">
          {t(($) => $.web.desktop_handoff.invalid_app_origin)}
        </p>
      </ClerkAuthShell>
    );
  }

  return (
    <ClerkAuthShell>
      <p role="status" aria-live="polite">
        {t(($) => $.callback.returning)}
      </p>
    </ClerkAuthShell>
  );
}
