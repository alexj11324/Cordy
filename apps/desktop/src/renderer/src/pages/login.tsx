import { useState } from "react";
import { api } from "@patchbay/core/api";
import { Alert, AlertDescription } from "@patchbay/ui/components/ui/alert";
import { LoginPage } from "@patchbay/views/auth";
import { useT } from "@patchbay/views/i18n";
import { DragStrip } from "@patchbay/views/platform";
import { PatchbayIcon } from "@patchbay/ui/components/common/patchbay-icon";
import { createDesktopGoogleLoginUrl } from "./login-handoff";

function requireRuntimeAccountsUrl(): string {
  const runtimeConfig = window.desktopAPI.runtimeConfig;
  if (!runtimeConfig.ok) {
    throw new Error(
      "Invariant violated: DesktopLoginPage rendered before App accepted runtime config",
    );
  }
  return runtimeConfig.config.accountsUrl;
}

export function DesktopLoginPage() {
  const accountsUrl = requireRuntimeAccountsUrl();
  const { t } = useT("auth");
  const [openingGoogle, setOpeningGoogle] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleGoogleLogin = async () => {
    if (openingGoogle) return;
    setOpeningGoogle(true);
    setError(null);
    try {
      const url = await createDesktopGoogleLoginUrl(
        accountsUrl,
        (state, codeChallenge) =>
          api.initiateDesktopAuthHandoff(state, codeChallenge),
      );
      await window.desktopAPI.openExternal(url);
    } catch {
      setError(t(($) => $.web.desktop_handoff.prepare_failed));
    } finally {
      setOpeningGoogle(false);
    }
  };

  return (
    <div className="flex h-screen flex-col">
      <DragStrip />
      {error && (
        <Alert variant="destructive" className="m-4">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      <LoginPage
        logo={<PatchbayIcon bordered size="lg" />}
        onSuccess={() => {
          // Auth store update triggers AppContent re-render → shows DesktopShell.
          // Initial workspace navigation happens in routes.tsx via IndexRedirect.
        }}
        onGoogleLogin={() => void handleGoogleLogin()}
      />
    </div>
  );
}
