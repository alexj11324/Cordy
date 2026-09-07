"use client";
import { AccountsLoginForm as SharedLoginForm } from '@patchbay/auth-ui/login-form';
import { useAuthMessages } from '@/lib/auth-messages';
import { resolveAccountsReturnUrl } from '@/lib/redirect';
import { useProductOrigin } from './runtime-clerk-provider';

export function buildGoogleLoginUrl(returnUrl: string, origin: string, productOrigin?: string): string {
  const destination = new URL(
    resolveAccountsReturnUrl(returnUrl, productOrigin),
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
 const productOrigin = useProductOrigin();
 return <SharedLoginForm messages={messages} onGoogleLogin={() => window.location.assign(buildGoogleLoginUrl(returnUrl, window.location.origin, productOrigin))} />;
}
