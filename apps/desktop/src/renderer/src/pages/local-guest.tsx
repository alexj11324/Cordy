import { useCallback, useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";
import { Button } from "@patchbay/ui/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@patchbay/ui/components/ui/card";
import { PatchbayIcon } from "@patchbay/ui/components/common/patchbay-icon";
import { useT } from "@patchbay/views/i18n";
import { DragStrip } from "@patchbay/views/platform";
import type { LocalRuntimeProbe } from "../../../shared/daemon-types";
import type { LocalGuestSession } from "../../../shared/local-guest";

type LocalGuestShellProps = {
  session: LocalGuestSession;
  onSwitchToCloud: () => Promise<void>;
  onExit: () => Promise<void>;
};

export function LocalGuestShell({
  session,
  onSwitchToCloud,
  onExit,
}: LocalGuestShellProps) {
  const { t } = useT("auth");
  const [directory, setDirectory] = useState<string | null>(null);
  const [directoryError, setDirectoryError] = useState(false);
  const [probe, setProbe] = useState<LocalRuntimeProbe | null>(null);
  const [runtimeLoading, setRuntimeLoading] = useState(false);
  const [cloudLoading, setCloudLoading] = useState(false);
  const [exitLoading, setExitLoading] = useState(false);
  const [actionError, setActionError] = useState(false);

  const refreshRuntimeProbe = useCallback(async () => {
    setRuntimeLoading(true);
    try {
      setProbe(await window.desktopAPI.probeLocalRuntimes());
    } catch {
      setProbe({ probeResult: "error" });
    } finally {
      setRuntimeLoading(false);
    }
  }, []);

  useEffect(() => {
    void refreshRuntimeProbe();
  }, [refreshRuntimeProbe]);

  const chooseDirectory = async () => {
    setDirectoryError(false);
    const picked = await window.desktopAPI.pickDirectory(directory ?? undefined);
    if (!picked.ok || !picked.path) return;

    const validation = await window.desktopAPI.validateLocalDirectory(picked.path);
    if (!validation.ok) {
      setDirectoryError(true);
      return;
    }
    setDirectory(picked.path);
  };

  const switchToCloud = async () => {
    if (cloudLoading) return;
    setCloudLoading(true);
    setActionError(false);
    try {
      await onSwitchToCloud();
    } catch {
      setActionError(true);
    } finally {
      setCloudLoading(false);
    }
  };

  const exitGuest = async () => {
    if (exitLoading) return;
    setExitLoading(true);
    setActionError(false);
    try {
      await onExit();
    } catch {
      setActionError(true);
    } finally {
      setExitLoading(false);
    }
  };

  return (
    <div className="flex h-screen flex-col bg-page-canvas text-foreground">
      <DragStrip />
      <main className="min-h-0 flex-1 overflow-auto p-6">
        <div className="mx-auto flex w-full max-w-3xl flex-col gap-6">
          <header className="flex items-center gap-3">
            <PatchbayIcon bordered size="md" />
            <div>
              <h1 className="text-title font-semibold">
                {t(($) => $.guest.shell_title)}
              </h1>
              <p className="text-body text-muted-foreground">
                {t(($) => $.guest.local_only)}
              </p>
            </div>
          </header>

          <Card>
            <CardHeader>
              <CardTitle>{t(($) => $.guest.identity)}</CardTitle>
              <CardDescription>
                {t(($) => $.guest.identity_description)}
              </CardDescription>
            </CardHeader>
            <CardContent>
              <div className="rounded-lg border border-surface-border bg-surface-hover px-3 py-2 text-body">
                {session.displayName}
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>{t(($) => $.guest.directory_title)}</CardTitle>
              <CardDescription>
                {t(($) => $.guest.directory_description)}
              </CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-3">
              <Button
                type="button"
                variant="outline"
                className="self-start"
                onClick={() => void chooseDirectory()}
              >
                {t(($) => $.guest.choose_directory)}
              </Button>
              {directory && (
                <p className="break-all text-caption text-muted-foreground">
                  <span className="font-medium text-foreground">
                    {t(($) => $.guest.directory_selected)}: {" "}
                  </span>
                  {directory}
                </p>
              )}
              {directoryError && (
                <p role="alert" className="text-caption text-destructive">
                  {t(($) => $.guest.directory_invalid)}
                </p>
              )}
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <div className="flex items-start justify-between gap-3">
                <div className="space-y-1">
                  <CardTitle>{t(($) => $.guest.runtime_title)}</CardTitle>
                  <CardDescription>
                    {t(($) => $.guest.runtime_description)}
                  </CardDescription>
                </div>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  aria-label={t(($) => $.guest.runtime_refresh)}
                  disabled={runtimeLoading}
                  onClick={() => void refreshRuntimeProbe()}
                >
                  <RefreshCw
                    className={runtimeLoading ? "animate-spin" : undefined}
                  />
                </Button>
              </div>
            </CardHeader>
            <CardContent>
              {runtimeLoading && !probe ? (
                <p className="text-body text-muted-foreground">
                  {t(($) => $.guest.runtime_loading)}
                </p>
              ) : probe?.probeResult === "success" ? (
                <div className="space-y-3">
                  <div className="flex flex-wrap gap-x-4 gap-y-1 text-caption text-muted-foreground">
                    <span>
                      {t(($) => $.guest.runtime_count, {
                        count: probe.runtimeCount,
                      })}
                    </span>
                    <span>
                      {t(($) => $.guest.runtime_online, {
                        count: probe.onlineCount,
                      })}
                    </span>
                    <span>
                      {t(($) => $.guest.runtime_offline, {
                        count: probe.offlineCount,
                      })}
                    </span>
                  </div>
                  {Object.keys(probe.providerSummary).length > 0 ? (
                    <ul className="grid gap-2 sm:grid-cols-2">
                      {Object.entries(probe.providerSummary).map(
                        ([provider, count]) => (
                          <li
                            key={provider}
                            className="flex items-center justify-between rounded-lg border border-surface-border px-3 py-2 text-body"
                          >
                            <code>{provider}</code>
                            <span>{count}</span>
                          </li>
                        ),
                      )}
                    </ul>
                  ) : (
                    <p className="text-body text-muted-foreground">
                      {t(($) => $.guest.runtime_empty)}
                    </p>
                  )}
                </div>
              ) : (
                <p role="alert" className="text-body text-destructive">
                  {t(($) => $.guest.runtime_error)}
                </p>
              )}
            </CardContent>
          </Card>

          {actionError && (
            <p role="alert" className="text-body text-destructive">
              {t(($) => $.guest.unavailable)}
            </p>
          )}
          <div className="flex flex-wrap justify-end gap-2">
            <Button
              type="button"
              variant="outline"
              disabled={cloudLoading || exitLoading}
              onClick={() => void switchToCloud()}
            >
              {t(($) => $.guest.switch_to_signin)}
            </Button>
            <Button
              type="button"
              variant="ghost"
              disabled={cloudLoading || exitLoading}
              onClick={() => void exitGuest()}
            >
              {exitLoading
                ? t(($) => $.guest.exiting)
                : t(($) => $.guest.exit)}
            </Button>
          </div>
        </div>
      </main>
    </div>
  );
}

type GuestSessionRecoveryPageProps = {
  onReset: () => Promise<void>;
};

export function GuestSessionRecoveryPage({
  onReset,
}: GuestSessionRecoveryPageProps) {
  const { t } = useT("auth");
  const [resetting, setResetting] = useState(false);
  const [error, setError] = useState(false);

  const reset = async () => {
    if (resetting) return;
    setResetting(true);
    setError(false);
    try {
      await onReset();
    } catch {
      setError(true);
    } finally {
      setResetting(false);
    }
  };

  return (
    <div className="flex h-screen flex-col bg-page-canvas text-foreground">
      <DragStrip />
      <main className="flex flex-1 items-center justify-center p-6">
        <div className="flex max-w-sm flex-col items-center text-center">
          <PatchbayIcon bordered size="lg" />
          <h1 className="mt-6 text-title font-semibold">
            {t(($) => $.guest.session_error_title)}
          </h1>
          <p className="mt-2 text-body text-muted-foreground">
            {t(($) => $.guest.session_error)}
          </p>
          <Button className="mt-6" disabled={resetting} onClick={() => void reset()}>
            {resetting
              ? t(($) => $.guest.resetting)
              : t(($) => $.guest.reset)}
          </Button>
          {error && (
            <p role="alert" className="mt-3 text-caption text-destructive">
              {t(($) => $.guest.unavailable)}
            </p>
          )}
        </div>
      </main>
    </div>
  );
}
