import { useState } from "react";
import { useAuthStore } from "@patchbay/core/auth";
import { Alert, AlertDescription } from "@patchbay/ui/components/ui/alert";
import { Button } from "@patchbay/ui/components/ui/button";
import { PatchbayIcon } from "@patchbay/ui/components/common/patchbay-icon";
import { LoginPage } from "@patchbay/views/auth";
import { useT } from "@patchbay/views/i18n";
import { DragStrip } from "@patchbay/views/platform";
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
    } catch {
      setError(t(($) => $.desktop.entry.guest_error));
      setIsStarting(false);
    }
  };

  return (
    <div className="w-full space-y-2">
      <Button
        type="button"
        variant="ghost"
        className="h-auto w-full px-1 py-1 text-body text-muted-foreground"
        onClick={() => void handleContinue()}
        disabled={isStarting}
        aria-busy={isStarting}
      >
        {isStarting
          ? t(($) => $.desktop.entry.skipping)
          : t(($) => $.desktop.entry.skip)}
      </Button>
      {error && (
        <Alert variant="destructive" aria-live="polite">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
    </div>
  );
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
      const url = await createDesktopGoogleLoginUrl(accountsUrl);
      await window.desktopAPI.openExternal(url);
    } catch {
      setError(t(($) => $.desktop.entry.login_error));
    } finally {
      setOpeningGoogle(false);
    }
  };

  return (
    <div className="flex h-screen flex-col bg-background">
      <DragStrip />
      <main
        id="desktop-login"
        className="flex min-h-0 flex-1 flex-col overflow-y-auto"
      >
        <div
          data-testid="authentication-example"
          className="relative grid min-h-0 w-full flex-1 grid-cols-2"
        >
          <div
            data-testid="authentication-brand-panel"
            className="relative flex h-full min-h-0 flex-col p-10 text-primary dark:border-r"
          >
            <div className="absolute inset-0 bg-primary/5" aria-hidden="true" />
            <div className="relative z-20 flex items-center text-title font-medium">
              <PatchbayIcon className="mr-2 h-6 w-6" noSpin />
              {t(($) => $.desktop.entry.brand)}
            </div>
            <div className="relative z-20 mt-auto">
              <blockquote className="leading-normal text-balance">
                {t(($) => $.desktop.entry.quote)}
              </blockquote>
            </div>
          </div>
          <div
            data-testid="authentication-form-panel"
            className="flex h-full min-h-0 items-center justify-center p-6 lg:p-8"
          >
            <LoginPage
              embedded
              externalError={
                error ? (
                  <Alert variant="destructive" aria-live="polite">
                    <AlertDescription>{error}</AlertDescription>
                  </Alert>
                ) : undefined
              }
              showGoogleSeparator
              googleLoading={openingGoogle}
              onGoogleLogin={() => void handleGoogleLogin()}
              onSuccess={() => undefined}
              extra={<GuestSessionEntry />}
            />
          </div>
        </div>
      </main>
    </div>
  );
}
