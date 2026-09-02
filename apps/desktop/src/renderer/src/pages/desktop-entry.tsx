import { useState, type FormEvent } from "react";
import { Button } from "@patchbay/ui/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@patchbay/ui/components/ui/card";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@patchbay/ui/components/ui/dialog";
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
  onEnableCloudMode: () => Promise<void>;
  onGuestSession: (session: LocalGuestSession) => void;
};

export function DesktopEntryPage({
  onEnableCloudMode,
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
      await onEnableCloudMode();
    } catch {
      setCloudError(true);
    } finally {
      setCloudSubmitting(false);
    }
  };

  return (
    <div className="flex h-screen flex-col bg-page-canvas text-foreground">
      <DragStrip />
      <main className="flex min-h-0 flex-1 items-center justify-center overflow-auto p-6">
        <Card className="w-full max-w-sm">
          <CardHeader className="items-center text-center">
            <PatchbayIcon bordered size="lg" />
            <CardTitle>{t(($) => $.signin.title)}</CardTitle>
            <CardDescription>
              {t(($) => $.guest.entry_description)}
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
            <Button
              type="button"
              variant="outline"
              disabled={cloudSubmitting}
              onClick={() => {
                void handleCloudSignIn();
              }}
            >
              {cloudSubmitting
                ? t(($) => $.guest.signin_loading)
                : t(($) => $.guest.signin_button)}
            </Button>
            <Button type="button" onClick={openGuestDialog}>
              {t(($) => $.guest.button)}
            </Button>
            {cloudError && (
              <p role="alert" className="text-caption text-destructive">
                {t(($) => $.guest.unavailable)}
              </p>
            )}
          </CardContent>
        </Card>
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
