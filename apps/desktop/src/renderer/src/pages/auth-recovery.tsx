import { useAuthStore } from "@patchbay/core/auth";
import { Button } from "@patchbay/ui/components/ui/button";
import { PatchbayIcon } from "@patchbay/ui/components/common/patchbay-icon";
import { useT } from "@patchbay/views/i18n";
import { DragStrip } from "@patchbay/views/platform";
import type { DaemonRecoveryReason } from "../../../shared/daemon-types";

export function DesktopAuthRecoveryPage({
  onRetry,
  isRetrying = false,
  errorReason,
}: {
  onRetry?: () => void;
  isRetrying?: boolean;
  /** Stable daemon/auth failure reason from the blocked startup gate. */
  errorReason?: DaemonRecoveryReason;
}) {
  const { t } = useT("auth");
  const retryAuthentication = useAuthStore(
    (state) => state.retryAuthentication,
  );
  const daemonError = (() => {
    switch (errorReason) {
      case "session_token_missing":
        return t(($) => $.desktop.recovery.daemon.session_token_missing);
      case "auto_start_disabled":
        return t(($) => $.desktop.recovery.daemon.auto_start_disabled);
      case "cli_not_found":
        return t(($) => $.desktop.recovery.daemon.cli_not_found);
      case "auth_expired":
        return t(($) => $.desktop.recovery.daemon.auth_expired);
      case "not_ready":
        return t(($) => $.desktop.recovery.daemon.not_ready);
      case "start_failed":
        return t(($) => $.desktop.recovery.daemon.start_failed);
      default:
        return null;
    }
  })();

  return (
    <div className="flex h-screen flex-col">
      <DragStrip />
      <div className="flex flex-1 items-center justify-center p-8">
        <div className="flex max-w-sm flex-col items-center text-center">
          <PatchbayIcon bordered size="lg" />
          <h1 className="mt-6 text-title font-semibold">
            {t(($) => $.desktop.recovery.title)}
          </h1>
          <p className="mt-2 text-body text-muted-foreground">
            {t(($) => $.desktop.recovery.description)}
          </p>
          {daemonError ? (
            <p
              className="mt-3 max-w-sm whitespace-pre-wrap text-caption text-destructive"
              role="alert"
            >
              {daemonError}
            </p>
          ) : null}
          <Button
            className="mt-6"
            disabled={isRetrying}
            onClick={onRetry ?? retryAuthentication}
          >
            {isRetrying
              ? t(($) => $.desktop.recovery.retrying)
              : t(($) => $.desktop.recovery.retry)}
          </Button>
        </div>
      </div>
    </div>
  );
}
