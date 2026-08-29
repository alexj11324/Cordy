import { LoginPage } from "@patchbay/views/auth";
import { DragStrip } from "@patchbay/views/platform";
import { PatchbayIcon } from "@patchbay/ui/components/common/patchbay-icon";
import { createDesktopLoginUrl } from "./login-handoff";

function requireRuntimeAppUrl(): string {
  const runtimeConfig = window.desktopAPI.runtimeConfig;
  if (!runtimeConfig.ok) {
    throw new Error(
      "Invariant violated: DesktopLoginPage rendered before App accepted runtime config",
    );
  }
  return runtimeConfig.config.appUrl;
}

export function DesktopLoginPage() {
  const webUrl = requireRuntimeAppUrl();
  const handleGoogleLogin = async () => {
    // Open web login page in the default browser with a PKCE-bound desktop
    // handoff. The web callback returns a one-time code, never the bearer.
    const url = await createDesktopLoginUrl(webUrl);
    await window.desktopAPI.openExternal(url);
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
      />
    </div>
  );
}
