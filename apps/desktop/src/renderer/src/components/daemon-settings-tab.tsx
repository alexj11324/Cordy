import { useState, useEffect, useCallback, type ReactNode } from "react";
import { AlertCircle, Info, LogIn } from "lucide-react";
import { Button } from "@lobehub/ui/base-ui";
import { cn } from "@orvilo/ui/lib/utils";
import { toast } from "sonner";
import {
  SettingsFormRow,
  SettingsGroup,
  SettingsSwitch,
} from "@orvilo/views/settings";
import { useT } from "@orvilo/views/i18n";
import { reauthenticateDaemon } from "../platform/daemon-reauth";
import type { DaemonPrefs, DaemonStatus } from "../../../shared/daemon-types";
import {
  DAEMON_STATE_COLORS,
  formatUptime,
} from "../../../shared/daemon-types";
import { daemonStateLabel } from "./daemon-i18n";

// One row inside the diagnostics block. Values that are likely to be
// long IDs / URLs render as monospaced + truncated with a tooltip.
//
// This is not a `SettingsFormRow`: it has no control column, and its label is a
// dimmed caption rather than the row's title. It keeps the two-column grid the
// old card used, inside the group that now supplies the card chrome — see
// `reference-lobe-tab-migration.md` on why a row whose label column carries its
// own layout stays hand-written.
function DiagnosticsRow({
  label,
  value,
  mono,
}: {
  label: string;
  value: ReactNode;
  mono?: boolean;
}) {
  return (
    <div className="grid grid-cols-[140px_minmax(0,1fr)] items-baseline gap-3 py-1.5">
      <span className="text-caption text-muted-foreground">{label}</span>
      <span
        className={cn(
          "min-w-0 truncate text-body",
          mono && "font-mono text-caption",
        )}
        title={typeof value === "string" ? value : undefined}
      >
        {value}
      </span>
    </div>
  );
}

/**
 * Desktop-only settings tab, migrated off the shadcn settings atoms onto the
 * Lobe shell the workspace tabs already use.
 *
 * What the migration owed this file, beyond the mechanical mapping:
 *
 * - **`SettingsTab` is gone, and the only thing it was contributing is the
 *   `space-y-12` its nested branch wraps children in.** That branch returns
 *   before it renders `title` / `description` / `action`, so this tab's page
 *   title and description have never appeared inside the settings dialog. The
 *   two banners keep their own `mt-4` untouched — they are not migrated atoms,
 *   and the new top-level `space-y-8` takes the place of the old wrapper only.
 *
 * - **Both of this tab's switches were unnamed, and that is what the required
 *   `label` on `SettingsSwitch` fixes.** Neither row passed `aria-label`, and
 *   `SettingsRow` only rendered a real `<label for>` when it was given an
 *   `htmlFor` — it was not. So the two controls that turn the background
 *   service on and off reached the accessibility tree as anonymous switches.
 *   They are the tab's primary interaction, not a detail.
 *
 * - **Both cards now carry a title, and neither reuses the tab's own.** The
 *   first card has been untitled since it was written; an untitled outlined
 *   `Form.Group` still renders a collapse header, so it would have opened with
 *   an empty band above its first row. `desktop.daemon.title` is not the
 *   replacement: the rail and the dialog header print the label from
 *   `extraAccountTabs`, which is the literal string "Daemon" — the same string
 *   that key resolves to in English.
 *
 * - **The diagnostics block keeps its own markup and loses its `px-4 py-2`.**
 *   `SettingsCard` drew no padding and the inner div supplied it; the group's
 *   body now carries `12px 16px`, so keeping the wrapper would have doubled it.
 */
export function DaemonSettingsTab() {
  const { t } = useT("settings");
  const [prefs, setPrefs] = useState<DaemonPrefs>({ autoStart: true, autoStop: false });
  const [cliInstalled, setCliInstalled] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState<DaemonStatus>({ state: "stopped" });
  const [reauthLoading, setReauthLoading] = useState(false);

  useEffect(() => {
    window.daemonAPI.getPrefs().then(setPrefs);
    window.daemonAPI.isCliInstalled().then(setCliInstalled);
    window.daemonAPI.getStatus().then(setStatus);
    return window.daemonAPI.onStatusChange(setStatus);
  }, []);

  const handleReauth = useCallback(async () => {
    setReauthLoading(true);
    await reauthenticateDaemon(t);
    setReauthLoading(false);
  }, [t]);

  const updatePref = useCallback(
    async (key: keyof DaemonPrefs, value: boolean) => {
      setSaving(true);
      try {
        const updated = await window.daemonAPI.setPrefs({ [key]: value });
        setPrefs(updated);
        toast.success(t(($) => $.desktop.daemon.settings_saved), {
          id: "settings-auto-save",
        });
      } catch (error) {
        toast.error(
          error instanceof Error
            ? error.message
            : t(($) => $.desktop.daemon.settings_save_failed),
        );
      } finally {
        setSaving(false);
      }
    },
    [t],
  );

  // The daemon runs somewhere the app can't drive (e.g. inside WSL2 behind a
  // Windows desktop): /health is reachable but the lifecycle CLI can't reach
  // its process. Auto-start/auto-stop can't work, so disable them and say why
  // rather than letting the toggles silently no-op. See #3916.
  const externallyManaged = status.externallyManaged === true;

  const autoStartLabel = t(($) => $.desktop.daemon.auto_start_title);
  const autoStopLabel = t(($) => $.desktop.daemon.auto_stop_title);

  return (
    <div className="space-y-8">
      {status.state === "auth_expired" && (
        <div className="mt-4 flex items-start gap-3 rounded-lg border border-destructive/40 bg-destructive/5 px-4 py-3">
          <AlertCircle className="mt-0.5 size-4 shrink-0 text-destructive" />
          <div className="min-w-0 flex-1">
            <p className="text-body font-medium text-destructive">
              {t(($) => $.desktop.daemon.signin_expired)}
            </p>
            <p className="mt-0.5 text-body text-muted-foreground">
              {t(($) => $.desktop.daemon.signin_expired_description)}
            </p>
          </div>
          {/* `SettingsPillButton active` resolved its tone to `primary` — the
              solid pill, which is Lobe's `type="primary"`; `shape="round"`
              carries the pill geometry. `icon` takes the node directly, the way
              the other migrated tabs pass it. */}
          <Button
            className="shrink-0"
            disabled={reauthLoading}
            icon={<LogIn className="size-4" />}
            shape="round"
            type="primary"
            onClick={handleReauth}
          >
            {t(($) => $.desktop.daemon.signin_again)}
          </Button>
        </div>
      )}

      {externallyManaged && (
        <div className="mt-4 flex items-start gap-3 rounded-lg border bg-muted/30 px-4 py-3">
          <Info className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
          <p className="min-w-0 text-body text-muted-foreground">
            {t(($) => $.desktop.daemon.external_description_before)}{" "}
            <code className="font-mono text-caption">orvilo daemon start</code> /{" "}
            <code className="font-mono text-caption">orvilo daemon stop</code>
            {t(($) => $.desktop.daemon.external_description_after)}
          </p>
        </div>
      )}

      <SettingsGroup title={t(($) => $.desktop.daemon.section_title)}>
        <SettingsFormRow
          label={autoStartLabel}
          description={t(($) => $.desktop.daemon.auto_start_description)}
        >
          <SettingsSwitch
            label={autoStartLabel}
            checked={prefs.autoStart}
            onCheckedChange={(checked) => updatePref("autoStart", checked)}
            disabled={saving || externallyManaged}
          />
        </SettingsFormRow>

        {/* `divider` is where the old `SettingsCard`'s `divide-y` went — the
            card ruled between its children and `Form.Item` rules only when
            asked. The first row never had a rule above it. */}
        <SettingsFormRow
          divider
          label={autoStopLabel}
          description={t(($) => $.desktop.daemon.auto_stop_description)}
        >
          <SettingsSwitch
            label={autoStopLabel}
            checked={prefs.autoStop}
            onCheckedChange={(checked) => updatePref("autoStop", checked)}
            disabled={saving || externallyManaged}
          />
        </SettingsFormRow>

        <SettingsFormRow
          divider
          label={t(($) => $.desktop.daemon.cli_status)}
          description={
            cliInstalled === null
              ? t(($) => $.desktop.daemon.cli_checking)
              : cliInstalled
                ? t(($) => $.desktop.daemon.cli_installed)
                : t(($) => $.desktop.daemon.cli_missing)
          }
        >
          {cliInstalled === false && (
            // `SettingsPillButton` with no `tone` — the `muted` pill, i.e.
            // Lobe's `fill`.
            <Button
              shape="round"
              type="fill"
              onClick={() =>
                window.desktopAPI.openExternal(
                  "https://github.com/alexj11324/Cordy#cli-installation",
                )
              }
            >
              {t(($) => $.desktop.daemon.installation_guide)}
            </Button>
          )}
          {cliInstalled !== false && <span />}
        </SettingsFormRow>
      </SettingsGroup>

      {/* Diagnostics — moved out of the logs panel so the panel can focus
          on logs. These fields matter for support tickets and bug reports,
          not for everyday use. */}
      <SettingsGroup
        title={t(($) => $.desktop.daemon.diagnostics_title)}
        description={t(($) => $.desktop.daemon.diagnostics_description)}
      >
        <DiagnosticsRow
          label={t(($) => $.desktop.daemon.state)}
          value={
            <span className="inline-flex items-center gap-1.5">
              <span
                className={cn(
                  "size-1.5 rounded-full",
                  DAEMON_STATE_COLORS[status.state],
                )}
              />
              {daemonStateLabel(status.state, t)}
            </span>
          }
        />
        <DiagnosticsRow
          label={t(($) => $.desktop.daemon.uptime)}
          value={status.uptime ? formatUptime(status.uptime) : "—"}
        />
        <DiagnosticsRow
          label="PID"
          value={status.pid ?? "—"}
          mono={!!status.pid}
        />
        <DiagnosticsRow
          label={t(($) => $.desktop.daemon.daemon_id)}
          value={status.daemonId ?? "—"}
          mono={!!status.daemonId}
        />
        <DiagnosticsRow
          label={t(($) => $.desktop.daemon.profile)}
          value={status.profile || "default"}
        />
        <DiagnosticsRow
          label={t(($) => $.desktop.daemon.server_url)}
          value={status.serverUrl ?? "—"}
          mono={!!status.serverUrl}
        />
        <DiagnosticsRow
          label={t(($) => $.desktop.daemon.device_name)}
          value={status.deviceName ?? "—"}
        />
        <DiagnosticsRow
          label={t(($) => $.desktop.daemon.workspaces)}
          value={
            typeof status.workspaceCount === "number"
              ? status.workspaceCount
              : "—"
          }
        />
      </SettingsGroup>
    </div>
  );
}
