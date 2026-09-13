import { useCallback, useEffect, useState } from "react";
import { AlertCircle, ArrowDownToLine, Check } from "lucide-react";
import { Button } from "@lobehub/ui/base-ui";
import { useT } from "@orvilo/views/i18n";
import {
  SettingsFormRow,
  SettingsGroup,
  SettingsSwitch,
} from "@orvilo/views/settings";
import { toast } from "sonner";

type CheckState =
  | { status: "idle" }
  | { status: "checking" }
  | { status: "up-to-date" }
  | { status: "available"; latestVersion: string }
  | { status: "error"; message: string };

/**
 * Desktop-only settings tab, migrated off the shadcn settings atoms onto the
 * Lobe shell the workspace tabs already use.
 *
 * Three things this file owed the migration, none of them mechanical:
 *
 * - **The `SettingsTab` wrapper is gone, and with it the `space-y-12` the
 *   nested branch put around the children.** The branch returns before it
 *   renders `title` / `description` / `action`, so inside the settings dialog
 *   this tab never showed a page title — the standalone header owns it. The
 *   card becomes one outlined `SettingsGroup`, and the top-level wrapper is
 *   `space-y-8`, the vertical rhythm the seven previously migrated tabs use.
 *
 * - **The section title is new copy, and deliberately not the tab's own.** The
 *   dialog header and the rail print `extraAccountTabs[].label`, which resolves
 *   to `desktop.tabs.updates` — and `desktop.updates.title` is the *same
 *   string* in both locales ("Updates" / "更新"). Promoting the old title would
 *   print it twice on one screen. A titleless outlined group is not an option
 *   either: Lobe's `Form.Group` always renders a collapse header, so the card
 *   would open with an empty band above its first row.
 *
 * - **The switch keeps the accessible name it already had.** Its `aria-label`
 *   came from the tab's own `Switch` re-export, which forwards `...props`;
 *   `SettingsSwitch` carries the same string through its required `label`.
 */
export function UpdatesSettingsTab() {
  const { t } = useT("settings");
  const [state, setState] = useState<CheckState>({ status: "idle" });
  const [automaticUpdates, setAutomaticUpdates] = useState(true);
  const [preferencesReady, setPreferencesReady] = useState(false);
  const [savingPreference, setSavingPreference] = useState(false);
  const currentVersion = window.desktopAPI.appInfo.version;

  useEffect(() => {
    let mounted = true;
    void window.updater
      .getPreferences()
      .then((preferences) => {
        if (mounted) setAutomaticUpdates(preferences.automaticUpdates);
      })
      .catch(() => {
        // The main process falls back to enabled when preferences cannot be
        // read. Keep the same safe default if IPC itself becomes unavailable.
      })
      .finally(() => {
        if (mounted) setPreferencesReady(true);
      });

    return () => {
      mounted = false;
    };
  }, []);

  const handleAutomaticUpdatesChange = useCallback(
    async (enabled: boolean) => {
      setSavingPreference(true);
      try {
        const preferences = await window.updater.setAutomaticUpdates(enabled);
        setAutomaticUpdates(preferences.automaticUpdates);
        toast.success(t(($) => $.auto_save.toast_saved), {
          id: "settings-auto-save",
        });
      } catch {
        toast.error(t(($) => $.desktop.updates.automatic_updates_save_failed));
      } finally {
        setSavingPreference(false);
      }
    },
    [t],
  );

  const handleCheck = useCallback(async () => {
    setState({ status: "checking" });
    const result = await window.updater.checkForUpdates();
    if (!result.ok) {
      setState({ status: "error", message: result.error });
      return;
    }
    setState(
      result.available
        ? { status: "available", latestVersion: result.latestVersion }
        : { status: "up-to-date" },
    );
  }, []);

  const checking = state.status === "checking";

  return (
    <div className="space-y-8">
      <SettingsGroup title={t(($) => $.desktop.updates.section_title)}>
        <SettingsFormRow label={t(($) => $.desktop.updates.current_version)}>
          <span className="font-mono text-caption text-muted-foreground">
            v{currentVersion}
          </span>
        </SettingsFormRow>

        {/* `divider` is where the old `SettingsCard`'s `divide-y` went: the card
            drew a rule between its children, and `Form.Item` only draws one
            when it is asked to. The first row never had one. */}
        <SettingsFormRow
          divider
          label={t(($) => $.desktop.updates.automatic_updates_title)}
          description={t(($) => $.desktop.updates.automatic_updates_description)}
        >
          {/* `label` is the same key the row prints, which is the name the old
              `aria-label` carried. A switch whose only name was the row text
              would be announced as an unnamed switch: antd's `Form.Item` label
              has no `for`, because a field with no `name` never mints one. */}
          <SettingsSwitch
            label={t(($) => $.desktop.updates.automatic_updates_title)}
            checked={automaticUpdates}
            onCheckedChange={handleAutomaticUpdatesChange}
            disabled={!preferencesReady || savingPreference}
          />
        </SettingsFormRow>

        <SettingsFormRow
          divider
          label={t(($) => $.desktop.updates.check_section_title)}
          align="start"
          description={
            <>
              <p>{t(($) => $.desktop.updates.check_section_description)}</p>
              {state.status === "up-to-date" && (
                <p className="mt-2 inline-flex items-center gap-1.5">
                  <Check className="size-3.5 text-success" />
                  {t(($) => $.desktop.updates.up_to_date)}
                </p>
              )}
              {state.status === "available" && (
                <p className="mt-2 inline-flex items-center gap-1.5">
                  <ArrowDownToLine className="size-3.5 text-primary" />
                  {t(($) => $.desktop.updates.downloading, {
                    version: state.latestVersion,
                  })}
                </p>
              )}
              {state.status === "error" && (
                <p className="mt-2 inline-flex items-center gap-1.5 text-destructive">
                  <AlertCircle className="size-3.5" />
                  {state.message}
                </p>
              )}
            </>
          }
        >
          {/* `SettingsPillButton` with no `tone` resolved to the `muted` pill —
              `bg-muted` on a transparent border, which is Lobe's `fill`, not
              its bordered default — and `shape="round"` carries the pill
              geometry. `loading` replaces the hand-rolled `Loader2` spinner.
              The old `disabled` is passed as well rather than left to
              `loading`: Lobe sets `aria-disabled` and swallows the click, which
              is behaviour parity, but only the prop puts the native attribute
              back on the element. */}
          <Button
            disabled={checking}
            loading={checking}
            shape="round"
            type="fill"
            onClick={handleCheck}
          >
            {checking
              ? t(($) => $.desktop.updates.checking)
              : t(($) => $.desktop.updates.check_now)}
          </Button>
        </SettingsFormRow>
      </SettingsGroup>
    </div>
  );
}
