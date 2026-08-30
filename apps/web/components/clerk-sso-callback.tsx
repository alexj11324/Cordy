"use client";

import { AuthenticateWithRedirectCallback } from "@clerk/nextjs";
import { useT } from "@patchbay/views/i18n";
import { ClerkAuthShell } from "@/components/clerk-auth-shell";
import {
  buildDesktopHandoffQuery,
  hasInvalidDesktopBrowserAppOrigin,
} from "@/features/auth/desktop-handoff";
import {
  authRouteWithRedirect,
  resolveSafeRedirectUrl,
} from "@/features/auth/safe-redirect";
import { useWebSearchParams } from "@/platform/client-navigation";

type ClerkSSOCallbackProps = {
  signInPath: "/login" | "/sign-in";
  signUpPath: "/signup" | "/sign-up";
};

export function ClerkSSOCallback({
  signInPath,
  signUpPath,
}: ClerkSSOCallbackProps) {
  const searchParams = useWebSearchParams();
  const { t } = useT("auth");
  const desktopHandoff = searchParams.get("platform") === "desktop";
  if (
    desktopHandoff &&
    hasInvalidDesktopBrowserAppOrigin(
      searchParams,
      process.env.NEXT_PUBLIC_DESKTOP_APP_ORIGIN,
    )
  ) {
    return (
      <ClerkAuthShell>
        <p role="alert">
          {t(($) => $.web.desktop_handoff.invalid_app_origin)}
        </p>
      </ClerkAuthShell>
    );
  }

  const desktopHandoffQuery = desktopHandoff
    ? buildDesktopHandoffQuery(
        searchParams,
        process.env.NEXT_PUBLIC_DESKTOP_APP_ORIGIN,
      )
    : "";
  const redirectUrl = resolveSafeRedirectUrl(searchParams.get("redirect_url"));
  const loginUrl = desktopHandoff
    ? `${signInPath}?${desktopHandoffQuery}`
    : authRouteWithRedirect(signInPath, redirectUrl);
  const signUpUrl = desktopHandoff
    ? `${signUpPath}?${desktopHandoffQuery}`
    : authRouteWithRedirect(signUpPath, redirectUrl);
  const returnUrl = desktopHandoff
    ? `/login?${desktopHandoffQuery}`
    : redirectUrl;

  return (
    <AuthenticateWithRedirectCallback
      signInUrl={loginUrl}
      signUpUrl={signUpUrl}
      signInFallbackRedirectUrl={returnUrl}
      signUpFallbackRedirectUrl={returnUrl}
    />
  );
}
