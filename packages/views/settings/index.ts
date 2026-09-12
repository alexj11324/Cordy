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
// The four exports below are the ones that cannot be a bare Lobe component, for
// two different reasons: `SettingsSwitch` and `SettingsSelect` lose the
// accessible name (or the search and windowing) of the component that wraps
// them, and `SettingsEmptyState` / `SettingsSearchBar` sit behind a deep import
// because `Empty` and `SearchBar` are reachable only from the root barrel,
// which is the ~11s import no tab may pay. Each module's header says which.
export { SettingsSwitch } from "./components/settings-switch";
export type { SettingsSwitchProps } from "./components/settings-switch";
export { SettingsEmptyState } from "./components/settings-empty";
export type { SettingsEmptyStateProps } from "./components/settings-empty";
export { SettingsSearchBar } from "./components/settings-search";
export type { SettingsSearchBarProps } from "./components/settings-search";
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
