import { useState } from "react";
import { useAuthStore } from "@patchbay/core/auth";
import { Button } from "@patchbay/ui/components/ui/button";
import { PatchbayIcon } from "@patchbay/ui/components/common/patchbay-icon";
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

/**
 * Desktop owns this entry screen. The formal login is a browser handoff to
 * the web Clerk flow; the app never renders Clerk, Connect, or a web login
 * form inside Electron. Skipping creates a real server-backed guest session.
 */
export function DesktopLoginPage() {
  const { t } = useT("auth");
  const createGuestSession = useAuthStore((state) => state.createGuestSession);
  const [isSkipping, setIsSkipping] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleLogin = () => {
    setError(null);
    try {
      void Promise.resolve(
        window.desktopAPI.openExternal(
          `${requireRuntimeAppUrl()}/login?platform=desktop`,
        ),
      ).catch(() => {
        setError(t(($) => $.desktop.entry.login_error));
      });
    } catch {
      setError(t(($) => $.desktop.entry.login_error));
    }
  };

  const handleSkip = async () => {
    if (isSkipping) return;
    setError(null);
    setIsSkipping(true);
    try {
      const user = await createGuestSession();
      if (user.is_guest !== true) {
        throw new Error("The server did not return a guest session");
      }
      // The auth store persists the real bearer token and publishes the real
      // server User. AppContent then follows the normal API-backed onboarding
      // path; no preview user or local-only screen is involved.
    } catch {
      setError(t(($) => $.desktop.entry.guest_error));
      setIsSkipping(false);
    }
  };

  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <DragStrip />
      <main className="flex min-h-0 flex-1 items-center justify-center px-6 py-12">
        <section className="flex w-full max-w-sm flex-col items-center text-center">
          <PatchbayIcon bordered size="lg" />
          <h1 className="mt-6 text-display-sm font-semibold">
            {t(($) => $.desktop.entry.title)}
          </h1>
          <p className="mt-3 text-body text-muted-foreground">
            {t(($) => $.desktop.entry.description)}
          </p>
          <div className="mt-8 flex w-full flex-col gap-3">
            <Button type="button" size="lg" className="w-full" onClick={handleLogin}>
              {t(($) => $.desktop.entry.login)}
            </Button>
            <Button
              type="button"
              size="lg"
              variant="outline"
              className="w-full"
              onClick={() => void handleSkip()}
              disabled={isSkipping}
              aria-busy={isSkipping}
            >
              {isSkipping
                ? t(($) => $.desktop.entry.skipping)
                : t(($) => $.desktop.entry.skip)}
            </Button>
          </div>
          {error && (
            <p className="mt-4 text-body text-destructive" role="alert">
              {error}
            </p>
          )}
        </section>
      </main>
    </div>
  );
}
