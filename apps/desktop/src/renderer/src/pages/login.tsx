import { useState } from "react";
import { api } from "@patchbay/core/api";
import { Alert, AlertDescription } from "@patchbay/ui/components/ui/alert";
import { Button } from "@patchbay/ui/components/ui/button";
import { PatchbayIcon } from "@patchbay/ui/components/common/patchbay-icon";
import { useT } from "@patchbay/views/i18n";
import { DragStrip } from "@patchbay/views/platform";
import { createDesktopLoginUrl } from "./login-handoff";

function requireRuntimeAccountsUrl(): string {
  const runtimeConfig = window.desktopAPI.runtimeConfig;
  if (!runtimeConfig.ok) {
    throw new Error(
      "Invariant violated: DesktopLoginPage rendered before App accepted runtime config",
    );
  }
  return runtimeConfig.config.accountsUrl;
}

/**
 * Desktop owns only the native handoff boundary. All Clerk UI lives on the
 * Accounts origin so browser auth, cookies, and recovery stay in one place.
 */
export function DesktopLoginPage() {
  const accountsUrl = requireRuntimeAccountsUrl();
  const { t } = useT("auth");
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState(false);

  const openSignIn = async () => {
    if (opening) return;
    setOpening(true);
    setError(false);
    try {
      const url = await createDesktopLoginUrl(
        accountsUrl,
        (state, codeChallenge) =>
          api.initiateDesktopAuthHandoff(state, codeChallenge),
      );
      await window.desktopAPI.openExternal(url);
    } catch {
      setError(true);
    } finally {
      setOpening(false);
    }
  };

  return (
    <div
      data-testid="desktop-login-pending"
      className="flex h-screen flex-col bg-zinc-950 text-white"
    >
      <DragStrip />
      <main className="flex min-h-0 flex-1 items-center justify-center overflow-auto px-8 py-12">
        <div className="flex w-full max-w-md -translate-y-[4vh] flex-col items-center text-center">
          <PatchbayIcon className="size-16 text-white" noSpin />
          <h1 className="mt-8 text-display-sm font-semibold tracking-tight">
            {t(($) => $.desktop.entry.browser_title)}
          </h1>
          <p className="mt-3 text-body leading-relaxed text-zinc-400">
            {t(($) => $.desktop.entry.browser_description)}
          </p>
          <Button
            type="button"
            className="mt-8 h-11 rounded-full bg-white px-6 text-zinc-950 hover:bg-zinc-200"
            disabled={opening}
            aria-busy={opening}
            onClick={() => void openSignIn()}
          >
            {opening
              ? t(($) => $.desktop.entry.browser_opening)
              : t(($) => $.desktop.entry.browser_button)}
          </Button>
          {error && (
            <Alert
              variant="destructive"
              className="mt-5 border-red-900 bg-red-950/40 text-left"
              aria-live="polite"
            >
              <AlertDescription>
                {t(($) => $.desktop.entry.login_error)}
              </AlertDescription>
            </Alert>
          )}
        </div>
      </main>
    </div>
  );
}
