"use client";

import { useRouter, useSearchParams } from "next/navigation";
import { Suspense, useEffect } from "react";
import { ClerkAuthShell } from "@/components/clerk-auth-shell";
import { useT } from "@patchbay/views/i18n";

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
  const router = useRouter();
  const searchParams = useSearchParams();
  const { t } = useT("auth");

  useEffect(() => {
    if (searchParams.get("platform") === "desktop") {
      router.replace("/login?platform=desktop");
      return;
    }
    const redirectUrl = searchParams.get("redirect_url");
    router.replace(resolveSafeRedirectUrl(redirectUrl));
  }, [router, searchParams]);

  return (
    <ClerkAuthShell>
      <p role="status" aria-live="polite">
        {t(($) => $.callback.returning)}
      </p>
    </ClerkAuthShell>
  );
}
