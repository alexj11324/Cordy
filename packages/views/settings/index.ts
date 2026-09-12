export { SettingsPage } from "./components";
export type { ExtraSettingsTab } from "./components";
export {
  SETTINGS_CONTROL_CLASS,
  SETTINGS_INLINE_FIELD_CLASS,
  SETTINGS_TEXTAREA_CLASS,
  SettingsCard,
  SettingsEmpty,
  SettingsField,
  SettingsIconButton,
  SettingsListRow,
  SettingsPillButton,
  SettingsRow,
  SettingsSaveState,
  SettingsSearchField,
  SettingsSection,
  SettingsSelectTrigger,
  SettingsTab,
  SettingsTextarea,
} from "./components/settings-layout";
export type {
  SettingsControlSize,
  SettingsSaveStatus,
} from "./components/settings-layout";
// The Lobe replacements, published here because the tab migrations live inside
// this package but the desktop tabs reach them through this barrel. The shell's
// row is `SettingsFormRow`, not `SettingsField`/`SettingsRow`: those names still
// belong to `settings-layout` until its last consumer is migrated.
export { SettingsGroup, SettingsFormRow } from "./components/settings-shell";
export type {
  SettingsFormRowProps,
  SettingsGroupProps,
} from "./components/settings-shell";
// The two controls that cannot be a bare Lobe component without losing either
// an accessible name or the current entry point; each module's header says
// which and why.
export { SettingsSwitch } from "./components/settings-switch";
export type { SettingsSwitchProps } from "./components/settings-switch";
export { SettingsSelect } from "./components/settings-select";
export type {
  SettingsSelectOption,
  SettingsSelectProps,
} from "./components/settings-select";
export { useSettingsConfirm } from "./components/settings-confirm";
export type {
  SettingsConfirmHandle,
  SettingsConfirmOptions,
} from "./components/settings-confirm";
