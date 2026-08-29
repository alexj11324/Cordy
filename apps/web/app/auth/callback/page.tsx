"use client";

import { useRouter } from "next/navigation";
import { useEffect } from "react";
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
  const router = useRouter();
  const { t } = useT("auth");

  useEffect(() => {
    const redirectUrl = new URLSearchParams(window.location.search).get(
      "redirect_url",
    );
    router.replace(resolveSafeRedirectUrl(redirectUrl));
  }, [router]);

  return (
    <ClerkAuthShell>
      <p role="status" aria-live="polite">
        {t(($) => $.callback.returning)}
      </p>
    </ClerkAuthShell>
  );
}
