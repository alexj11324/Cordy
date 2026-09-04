import { useState, type FormEvent } from "react";
import { Button } from "@patchbay/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@patchbay/ui/components/ui/dialog";
import { Input } from "@patchbay/ui/components/ui/input";
import { Label } from "@patchbay/ui/components/ui/label";
import { PatchbayIcon } from "@patchbay/ui/components/common/patchbay-icon";
import { useT } from "@patchbay/views/i18n";
import { DragStrip } from "@patchbay/views/platform";
import type {
  LocalGuestSession,
  GuestSessionMutationResult,
} from "../../../shared/local-guest";

type DesktopEntryPageProps = {
  onSignIn: () => Promise<void>;
  onGuestSession: (session: LocalGuestSession) => void;
};

export function DesktopEntryPage({
  onSignIn,
  onGuestSession,
}: DesktopEntryPageProps) {
  const { t } = useT("auth");
  const [guestDialogOpen, setGuestDialogOpen] = useState(false);
  const [displayName, setDisplayName] = useState("");
  const [guestError, setGuestError] = useState(false);
  const [guestSubmitting, setGuestSubmitting] = useState(false);
  const [cloudSubmitting, setCloudSubmitting] = useState(false);
  const [cloudError, setCloudError] = useState(false);

  const openGuestDialog = () => {
    setGuestError(false);
    setGuestDialogOpen(true);
  };

  const handleGuestSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (guestSubmitting) return;

    setGuestSubmitting(true);
    setGuestError(false);
    try {
      const result: GuestSessionMutationResult =
        await window.desktopAPI.createGuestSession(displayName);
      if (!result.ok) {
        setGuestError(true);
        return;
      }
      setGuestDialogOpen(false);
      setDisplayName("");
      onGuestSession(result.session);
    } catch {
      setGuestError(true);
    } finally {
      setGuestSubmitting(false);
    }
  };

  const handleCloudSignIn = async () => {
    if (cloudSubmitting) return;
    setCloudSubmitting(true);
    setCloudError(false);
    try {
      await onSignIn();
    } catch {
      setCloudError(true);
    } finally {
      setCloudSubmitting(false);
    }
  };

  return (
    <div
      data-testid="desktop-entry"
      className="flex h-screen flex-col bg-zinc-950 text-white"
    >
      <DragStrip />
      <main className="flex min-h-0 flex-1 items-center justify-center overflow-auto px-8 py-12">
        <div className="flex w-full max-w-2xl -translate-y-[4vh] flex-col items-center text-center">
          <div
            data-testid="desktop-entry-brand"
            className="flex items-center gap-4"
          >
            <PatchbayIcon
              className="size-16 shrink-0 text-white sm:size-20"
              noSpin
            />
            <h1 className="text-5xl font-semibold tracking-[-0.04em] text-white sm:text-6xl">
              Patchbay
            </h1>
          </div>
          <p className="mt-12 max-w-lg text-balance text-title leading-relaxed font-medium text-zinc-200 sm:text-display-sm">
            {t(($) => $.guest.hero_tagline)}
          </p>
          <div
            data-testid="desktop-entry-actions"
            className="mt-12 flex items-center justify-center gap-3"
          >
            <Button
              type="button"
              className="h-11 min-w-28 rounded-full bg-white px-6 text-zinc-950 transition-none hover:bg-zinc-200 active:not-aria-[haspopup]:translate-y-0 disabled:opacity-100"
              disabled={cloudSubmitting}
              aria-busy={cloudSubmitting}
              onClick={() => {
                void handleCloudSignIn();
              }}
            >
              {t(($) => $.guest.signin_button)}
            </Button>
            <Button
              type="button"
              variant="outline"
              className="h-11 min-w-28 rounded-full border-zinc-700 bg-zinc-900 px-6 text-white transition-none hover:bg-zinc-800 hover:text-white active:not-aria-[haspopup]:translate-y-0 disabled:opacity-100"
              disabled={cloudSubmitting}
              onClick={openGuestDialog}
            >
              {t(($) => $.guest.button)}
            </Button>
          </div>
          <div data-testid="desktop-entry-feedback" className="mt-4 min-h-5">
            {cloudError && (
              <p role="alert" className="text-caption text-red-400">
                {t(($) => $.desktop.entry.login_error)}
              </p>
            )}
          </div>
        </div>
      </main>

      <Dialog
        open={guestDialogOpen}
        onOpenChange={(open) => {
          if (!guestSubmitting) setGuestDialogOpen(open);
        }}
      >
        <DialogContent showCloseButton={!guestSubmitting}>
          <form onSubmit={(event) => void handleGuestSubmit(event)}>
            <DialogHeader>
              <DialogTitle>{t(($) => $.guest.name_prompt)}</DialogTitle>
              <DialogDescription>
                {t(($) => $.guest.name_description)}
              </DialogDescription>
            </DialogHeader>
            <div className="grid gap-2 py-4">
              <Label htmlFor="guest-display-name">
                {t(($) => $.guest.display_name_label)}
              </Label>
              <Input
                id="guest-display-name"
                autoFocus
                autoComplete="off"
                maxLength={64}
                value={displayName}
                onChange={(event) => setDisplayName(event.target.value)}
                placeholder={t(($) => $.guest.display_name_placeholder)}
                disabled={guestSubmitting}
                aria-invalid={guestError}
              />
              {guestError && (
                <p role="alert" className="text-caption text-destructive">
                  {t(($) => $.guest.invalid_name)}
                </p>
              )}
            </div>
            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                disabled={guestSubmitting}
                onClick={() => setGuestDialogOpen(false)}
              >
                {t(($) => $.guest.cancel)}
              </Button>
              <Button type="submit" disabled={guestSubmitting}>
                {guestSubmitting
                  ? t(($) => $.guest.creating)
                  : t(($) => $.guest.continue)}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </div>
  );
}
