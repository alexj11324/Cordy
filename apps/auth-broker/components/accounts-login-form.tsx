"use client";
import { AccountsLoginForm as SharedLoginForm } from '@patchbay/auth-ui/login-form';
import { useAuthMessages } from '@/lib/auth-messages';
import { resolveAccountsReturnUrl } from '@/lib/redirect';

export function buildGoogleLoginUrl(returnUrl: string, origin: string): string {
  const destination = new URL(
    resolveAccountsReturnUrl(returnUrl),
    origin,
  );
  const url = new URL("/oauth/google", origin);
  if (destination.searchParams.get("platform") === "desktop") {
    url.search = destination.search;
  } else {
    url.searchParams.set("return_url", destination.href);
  }
  return url.href;
}


export function AccountsLoginForm({returnUrl}: {returnUrl: string}) {
 const messages = useAuthMessages();
 return <SharedLoginForm messages={messages} onGoogleLogin={() => window.location.assign(buildGoogleLoginUrl(returnUrl, window.location.origin))} />;
}
