import { useState } from "react";
import { Mail } from "lucide-react";
import { useAuthStore } from "@patchbay/core/auth";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@patchbay/ui/components/ui/card";
import { Alert, AlertDescription } from "@patchbay/ui/components/ui/alert";
import { Button } from "@patchbay/ui/components/ui/button";
import { PatchbayIcon } from "@patchbay/ui/components/common/patchbay-icon";
import { GoogleIcon } from "@patchbay/views/onboarding/components/brand-icons";
import { LoginPage } from "@patchbay/views/auth";
import { buildDesktopGoogleLoginUrl } from "./login-url";
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
      {error && (
        <Alert variant="destructive" aria-live="polite">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
    </div>
  );
}

export function DesktopLoginPage() {
  const webUrl = requireRuntimeAppUrl();
  const { t } = useT("auth");
  const [showEmailFlow, setShowEmailFlow] = useState(false);
  const [openingMethod, setOpeningMethod] = useState<"google" | "email" | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);

  if (showEmailFlow) {
    return (
      <div className="flex h-screen flex-col bg-background">
        <DragStrip />
        <main className="flex min-h-0 flex-1 items-center justify-center overflow-y-auto px-4 py-10">
          <LoginPage
            logo={<PatchbayIcon bordered size="lg" />}
            onSuccess={() => undefined}
            extra={
              <Button
                type="button"
                variant="ghost"
                className="w-full"
                onClick={() => setShowEmailFlow(false)}
              >
                {t(($) => $.common.back)}
              </Button>
            }
          />
        </main>
      </div>
    );
  }

  const handleLogin = async (method: "google" | "email") => {
    if (method === "email") {
      setShowEmailFlow(true);
      return;
    }
    if (openingMethod) return;
    setOpeningMethod(method);
    setError(null);
    try {
      // Keep Clerk's hosted sign-in in the system browser. The browser-side
      // callback returns through the allowlisted patchbay:// handoff, where
      // the desktop renderer exchanges the short-lived credential server-side.
      await window.desktopAPI.openExternal(buildDesktopGoogleLoginUrl(webUrl));
    } catch {
      setError(t(($) => $.desktop.entry.login_error));
    } finally {
      setOpeningMethod(null);
    }
  };

  return (
    <div className="flex h-screen flex-col bg-background">
      <DragStrip />
      <main className="flex min-h-0 flex-1 items-center justify-center overflow-y-auto px-4 py-10">
        <Card className="w-full max-w-md shadow-lg">
          <CardHeader className="items-center text-center">
            <PatchbayIcon bordered size="lg" />
            <CardTitle className="text-display-sm">
              {t(($) => $.desktop.entry.title)}
            </CardTitle>
            <CardDescription className="max-w-sm text-pretty">
              {t(($) => $.desktop.entry.description)}
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            {error && (
              <Alert variant="destructive" aria-live="polite">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
            <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
              <Button
                type="button"
                className="w-full"
                size="lg"
                onClick={() => void handleLogin("google")}
                disabled={openingMethod !== null}
                aria-busy={openingMethod === "google"}
              >
                <GoogleIcon className="size-4" />
                {openingMethod === "google"
                  ? t(($) => $.desktop.entry.opening_google)
                  : t(($) => $.desktop.entry.login_google)}
              </Button>
              <Button
                type="button"
                variant="outline"
                className="w-full"
                onClick={() => void handleLogin("email")}
                disabled={openingMethod !== null}
                aria-busy={openingMethod === "email"}
              >
                <Mail className="size-4" aria-hidden="true" />
                {openingMethod === "email"
                  ? t(($) => $.desktop.entry.opening_email)
                  : t(($) => $.desktop.entry.login_email)}
              </Button>
            </div>
          </CardContent>
          <CardFooter className="flex-col gap-3">
            <GuestSessionEntry />
          </CardFooter>
        </Card>
      </main>
    </div>
  );
}
