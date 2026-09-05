"use client";

import { AccountsLoginForm } from "@patchbay/auth-ui/login-form";
import { messagesForLocale } from "@patchbay/auth-ui/messages";
import { useClerk } from "@clerk/nextjs";
import { useLocale } from "@patchbay/views/i18n";
import { authRouteWithRedirect } from "@/features/auth/safe-redirect";

export function WebAccountsLoginForm({ returnUrl }: { returnUrl: string }) {
  const clerk = useClerk();
  const messages = messagesForLocale(useLocale());
  return (
    <div className="w-full max-w-sm">
      <AccountsLoginForm messages={messages} onGoogleLogin={async () => {
        const client = clerk.client;
        if (!client) throw new Error(messages.unavailable);
        client.resetSignIn();
        const result = await client.signIn.__internal_future.sso({
          strategy: "oauth_google",
          redirectCallbackUrl: `/sso-callback?${new URLSearchParams({ redirect_url: returnUrl })}`,
          redirectUrl: authRouteWithRedirect("/login", returnUrl),
        });
        if (result.error) throw result.error;
      }} />
    </div>
  );
}
