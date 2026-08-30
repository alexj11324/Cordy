"use client";

import { Suspense } from "react";
import { SignIn } from "@clerk/nextjs";
import { useSearchParams } from "next/navigation";
import { ClerkAuthShell } from "@/components/clerk-auth-shell";
import {
  buildDesktopHandoffQuery,
  readDesktopBrowserAppOrigin,
} from "@/features/auth/desktop-handoff";
import {
  authRouteWithRedirect,
  resolveSafeRedirectUrl,
} from "@/features/auth/safe-redirect";
import { useT } from "@patchbay/views/i18n";

export default function SignInPage() {
  return (
    <Suspense>
      <SignInContent />
    </Suspense>
  );
}

function SignInContent() {
  const searchParams = useSearchParams();
  const { t } = useT("auth");
  const desktopHandoff = searchParams.get("platform") === "desktop";
  const requestedAppOrigin = searchParams.get("app_origin");
  const browserAppOrigin = readDesktopBrowserAppOrigin(
    searchParams,
    process.env.NEXT_PUBLIC_DESKTOP_APP_ORIGIN,
  );
  if (desktopHandoff && requestedAppOrigin !== null && !browserAppOrigin) {
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
  return (
    <ClerkAuthShell>
      <SignIn
        routing="path"
        path="/sign-in"
        signUpUrl={
          desktopHandoff
            ? `/sign-up?${desktopHandoffQuery}`
            : authRouteWithRedirect("/sign-up", redirectUrl)
        }
        fallbackRedirectUrl={
          desktopHandoff ? `/login?${desktopHandoffQuery}` : redirectUrl
        }
      />
    </ClerkAuthShell>
  );
}
