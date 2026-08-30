import { useState } from "react";
import { useAuthStore } from "@patchbay/core/auth";
import { Button } from "@patchbay/ui/components/ui/button";
import { LoginPage } from "@patchbay/views/auth";
import { PatchbayIcon } from "@patchbay/ui/components/common/patchbay-icon";
import { buildDesktopLoginUrl } from "./login-url";
import { useT } from "@patchbay/views/i18n";
import { DragStrip } from "@patchbay/views/platform";

function requireRuntimeAppUrl(): string {
  const runtimeConfig = window.desktopAPI.runtimeConfig;
  if (!runtimeConfig.ok) {
    throw new Error(
      "Invariant violated: DesktopLoginPage rendered before App accepted runtime config",
    );
  }
  return runtimeConfig.config.appUrl;
}

function GuestSessionEntry() {
  const { t } = useT("auth");
  const createGuestSession = useAuthStore((state) => state.createGuestSession);
  const [isStarting, setIsStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleContinue = async () => {
    if (isStarting) return;
    setIsStarting(true);
    setError(null);
    try {
      const user = await createGuestSession();
      if (user.is_guest !== true) {
        throw new Error("The server did not return a guest session");
      }
      // The auth store persists the real bearer token and publishes the real
      // server User. AppContent then follows the normal API-backed route.
    } catch {
      setError(t(($) => $.desktop.entry.guest_error));
      setIsStarting(false);
    }
  };

  return (
    <div className="space-y-2">
      <Button
        type="button"
        variant="outline"
        className="w-full"
        size="lg"
        onClick={() => void handleContinue()}
        disabled={isStarting}
        aria-busy={isStarting}
      >
        {isStarting
          ? t(($) => $.desktop.entry.skipping)
          : t(($) => $.desktop.entry.skip)}
      </Button>
      {error && <p className="text-body text-destructive" role="alert">{error}</p>}
    </div>
  );
}

export function DesktopLoginPage() {
  const webUrl = requireRuntimeAppUrl();
  const handleGoogleLogin = () => {
    // Open web login page in the default browser with platform=desktop flag.
    // The web callback will redirect back via patchbay:// deep link with the token.
    window.desktopAPI.openExternal(buildDesktopLoginUrl(webUrl));
  };

  return (
    <div className="flex h-screen flex-col">
      <DragStrip />
      <LoginPage
        logo={<PatchbayIcon bordered size="lg" />}
        onSuccess={() => {
          // Auth store update triggers AppContent re-render → shows DesktopShell.
          // Initial workspace navigation happens in routes.tsx via IndexRedirect.
        }}
        onGoogleLogin={handleGoogleLogin}
        extra={<GuestSessionEntry />}
      />
    </div>
  );
}
