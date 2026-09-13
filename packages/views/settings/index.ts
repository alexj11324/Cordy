export { SettingsPage } from "./components";
export type { ExtraSettingsTab } from "./components";
// The save-state atom, rendered by three live tabs. It moved out of the atom
// layer before that layer was deleted; the status contract it reads never
// belonged to a layout module.
export { SettingsSaveState } from "./components/settings-save-state";
export type { SettingsSaveStatus } from "./components/settings-save-status";
// The Lobe replacements, published here because the tab migrations live inside
// this package but the desktop tabs reach them through this barrel. The shell's
// row is `SettingsFormRow`; the atom layer's `SettingsRow` and `SettingsField`
// went with `settings-layout.tsx`.
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
