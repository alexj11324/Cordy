"use client";

import {
  MANUAL_CREATE_FIELDS,
  QUICK_CREATE_FIELDS,
  useIssueCreateSettingsStore,
} from "@orvilo/core/issues/stores/issue-create-settings-store";
import { toast } from "sonner";
import { useT } from "../../i18n";
import { SettingsFormRow, SettingsGroup } from "./settings-shell";
import { SettingsSwitch } from "./settings-switch";

/**
 * Issue settings — its own tab under "My Account". One group per create-issue
 * mode (agent quick create / manual create), each a switch list of the fields
 * that mode keeps on its dialog toolbar. Persisted client-side per workspace;
 * a field toggled off stays reachable from the dialog's ⋯ overflow and
 * re-surfaces automatically while it holds a value, so hiding is never
 * destructive.
 *
 * Every switch is immediate-effect — it writes the store on change — so no row
 * carries a `name` and the group is layout only. The title and description the
 * old `SettingsTab` rendered now live on the dialog header in
 * `settings-page.tsx`.
 */
export function IssueTab() {
  const { t } = useT("settings");
  const quickFields = useIssueCreateSettingsStore((s) => s.quickCreateFields);
  const setQuickVisible = useIssueCreateSettingsStore((s) => s.setQuickCreateFieldVisible);
  const manualFields = useIssueCreateSettingsStore((s) => s.manualCreateFields);
  const setManualVisible = useIssueCreateSettingsStore((s) => s.setManualCreateFieldVisible);

  const savedToast = () =>
    toast.success(t(($) => $.auto_save.toast_saved), { id: "settings-auto-save" });

  return (
    <div className="space-y-8">
      <SettingsGroup
        title={t(($) => $.issue.quick_create_title)}
        description={t(($) => $.issue.quick_create_description)}
      >
        {QUICK_CREATE_FIELDS.map((field) => (
          <SettingsFormRow key={field} label={t(($) => $.issue.fields[field])}>
            <SettingsSwitch
              label={t(($) => $.issue.fields[field])}
              checked={quickFields.includes(field)}
              onCheckedChange={(checked) => {
                setQuickVisible(field, checked);
                savedToast();
              }}
            />
          </SettingsFormRow>
        ))}
      </SettingsGroup>

      <SettingsGroup
        title={t(($) => $.issue.manual_create_title)}
        description={t(($) => $.issue.manual_create_description)}
      >
        {MANUAL_CREATE_FIELDS.map((field) => (
          <SettingsFormRow key={field} label={t(($) => $.issue.fields[field])}>
            <SettingsSwitch
              label={t(($) => $.issue.fields[field])}
              checked={manualFields.includes(field)}
              onCheckedChange={(checked) => {
                setManualVisible(field, checked);
                savedToast();
              }}
            />
          </SettingsFormRow>
        ))}
      </SettingsGroup>
    </div>
  );
}
