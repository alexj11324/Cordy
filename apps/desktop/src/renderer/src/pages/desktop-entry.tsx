import { useState } from "react";
import { Button } from "@orvilo/ui/components/ui/button";
import { OrviloIcon } from "@orvilo/ui/components/common/orvilo-icon";
import { useT } from "@orvilo/views/i18n";
import { DragStrip } from "@orvilo/views/platform";

type DesktopEntryPageProps = {
  onSignIn: () => Promise<void>;
  onResetGuest?: () => Promise<void>;
  /** Mints an email-less cloud account and boots straight into the app. */
  onGuestSession: () => Promise<void>;
};

export function DesktopEntryPage({
  onSignIn,
  onGuestSession,
  onResetGuest,
}: DesktopEntryPageProps) {
  const { t } = useT("auth");
  // Which button is mid-flight, so only that one shows a busy state while both
  // stay disabled.
  const [pending, setPending] = useState<"cloud" | "guest" | "reset" | null>(null);
  const [failed, setFailed] = useState<"cloud" | "guest" | "reset" | null>(null);

  const run = async (which: "cloud" | "guest" | "reset", action: () => Promise<void>) => {
    if (pending) return;
    setPending(which);
    setFailed(null);
    try {
      await action();
    } catch {
      setFailed(which);
    } finally {
      setPending(null);
    }
  };

  return (
    <div
      data-testid="desktop-entry"
      className="dark flex h-screen flex-col bg-background text-foreground"
    >
      <DragStrip />
      <main className="flex min-h-0 flex-1 items-center justify-center overflow-auto px-8 py-12">
        <div className="flex w-full max-w-2xl -translate-y-[4vh] flex-col items-center text-center">
          <div
            data-testid="desktop-entry-brand"
            className="flex items-center gap-4"
          >
            <OrviloIcon
              className="size-16 shrink-0 text-foreground sm:size-20"
              noSpin
            />
            <h1 className="text-5xl font-semibold tracking-[-0.04em] text-foreground sm:text-6xl">
              Orvilo
            </h1>
          </div>
          <p className="mt-12 max-w-lg text-balance text-title leading-relaxed font-medium text-muted-foreground sm:text-display-sm">
            {t(($) => $.guest.hero_tagline)}
          </p>
          <div
            data-testid="desktop-entry-actions"
            className="mt-12 flex items-center justify-center gap-3"
          >
            <Button
              type="button"
              className="h-11 min-w-28 rounded-full px-6 transition-none active:not-aria-[haspopup]:translate-y-0 disabled:opacity-100"
              disabled={pending !== null}
              aria-busy={pending === "cloud"}
              onClick={() => {
                void run("cloud", onSignIn);
              }}
            >
              {t(($) => $.guest.signin_button)}
            </Button>
            <Button
              type="button"
              variant="outline"
              className="h-11 min-w-28 rounded-full px-6 transition-none active:not-aria-[haspopup]:translate-y-0 disabled:opacity-100"
              disabled={pending !== null}
              aria-busy={pending === "guest"}
              onClick={() => {
                void run("guest", onGuestSession);
              }}
            >
              {pending === "guest"
                ? t(($) => $.guest.creating)
                : t(($) => $.guest.button)}
            </Button>
          </div>
          <p className="mt-5 text-caption text-muted-foreground">
            {t(($) => $.guest.entry_description)}
          </p>
          {onResetGuest && (
            <div className="mt-4 text-center text-caption">
              <p>{t(($) => $.guest.session_error)}</p>
              <Button variant="link" disabled={pending !== null} onClick={() => { void run("reset", onResetGuest); }}>
                {t(($) => $.guest.reset)}
              </Button>
            </div>
          )}
          <div data-testid="desktop-entry-feedback" className="mt-4 min-h-5">
            {failed === "cloud" && (
              <p role="alert" className="text-caption text-destructive">
                {t(($) => $.desktop.entry.login_error)}
              </p>
            )}
            {(failed === "guest" || failed === "reset") && (
              <p role="alert" className="text-caption text-destructive">
                {t(($) => $.guest.unavailable)}
              </p>
            )}
          </div>
        </div>
      </main>
    </div>
  );
}
